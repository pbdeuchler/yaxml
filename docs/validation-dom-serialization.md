# Complete DOM serialization validation

This report covers the complete borrowed/owned DOM serialization milestone
through production and documentation commit `b181960`, followed by the
artifact-only holistic review integrated as `b439ea8`. It does not claim the
remaining libxml2 save surface, a matched performance win, sustained
coverage-guided fuzzing, or 100% execution coverage.

## Implemented contract

`Writer::write_document` and `Writer::write_owned_document` require a fresh
writer, traverse a complete immutable tree iteratively, validate every generated
event through the existing writer state, flush the sink, and leave the writer
finished. The traversal holds one reusable, fallibly grown attribute vector and
borrows all names, namespace URIs, normalized values, and owned-pool strings.

Output is compact UTF-8 without a BOM or inserted whitespace. Declaration
absence, version, and standalone are retained; a present encoding becomes
`UTF-8`. Explicit namespace declarations, attribute order, document-child order,
mixed content, and empty CDATA/comment/PI nodes remain distinct. Childless
elements use empty-element spelling because the DOM does not retain the source's
start/end distinction. Namespace repair, detached subtrees, formatting, save
options, and UTF-16 output remain separate work.

## Behavioral and security evidence

The restored final tree passed 200 all-target tests; six subprocess helper tests
are intentionally ignored and launched by their parent. Five doctests,
formatting, strict all-target Clippy, `git diff --check`, the memchr-only
dependency audit, and the unsafe-code audit passed. The independent security
review found no Critical, Important, or Minor issue in traversal, allocation,
validation, poisoning, I/O, or position accounting.

Ten public serialization tests add independent byte goldens, strict complete
tree snapshots, 112 generated idempotent round trips, UTF-8/BOM/UTF-16 owned
inputs, all six Writer ceilings below/exact/above, output-coordinate failures,
fresh/finished misuse, short/Interrupted/zero/oversized/erroring/panicking sinks,
and no I/O after terminal results. Depth 8,192 succeeds on a 256 KiB stack and
10,000 siblings retain their order. Strict comparisons include node kinds and
exhaustion, topology, qualified/expanded names, ordered XMLNS and ordinary
attributes, PI targets, payloads, and declaration fields; source ranges and the
unretained empty-element spelling are intentionally excluded.

The [holistic review](review-dom-serialization.md) read every changed file and
caught 12 isolated mutations: empty and constant output, missing end tags, empty
CDATA loss, unused namespace loss, UTF-16 label forwarding, Writer-local
cumulative-budget reset and double charge, blanket borrowed and owned
`OwnedBytes` rejection, swallowed final-flush failure, and recursive traversal.
The accounting and blanket-owned mutations were killed by the new fuzz
assertions on independently decoded UTF-16 as well as borrowed input. Every
mutation was restored before the final suite.

## Fuzz target and source coverage

The dependency-free driver maps both complete-document methods. Accepted trees
must serialize, fully reparse, and match strict structure. Inputs at most 256
bytes cannot be dismissed through a blanket storage error. Seven independent
goldens vary output, token, event, attribute, depth, and cumulative owned-string
ceilings below/exact/above. Scheduled sinks cover partial writes, interruption,
invalid counts, prefix failures, final flush failures, confirmed positions, and
terminal no-I/O behavior. The external replay tool also evaluates serialized
bytes with the original bounded libxml2 projection; the stricter in-process
oracle covers fields that XPath projection omits.

Both coverage runs use default features, debug optimization, nightly
`2026-06-06` (Rust 1.98.0-nightly / LLVM 22.1.6), and
`-C instrument-coverage -Z coverage-options=branch`. Apple LLVM 21.0.0 reads the
profiles. Reports exclude dependency, toolchain, test, fuzz, example, and
benchmark sources; no production region is excluded.

| Run | Regions | Functions | Lines | Instrumented branch outcomes |
| --- | --- | --- | --- | --- |
| Fuzz smoke only | 4,130 / 4,329 (95.40%) | 272 / 294 (92.52%) | 2,736 / 2,804 (97.57%) | 588 / 622 (94.53%) |
| Ordinary tests only | 4,214 / 4,402 (95.73%) | 280 / 300 (93.33%) | 2,809 / 2,864 (98.08%) | 601 / 628 (95.70%) |

Under fuzz smoke, `serialize.rs` reaches 202 / 218 regions, 11 / 12
functions, 136 / 143 lines, and 17 / 20 branch outcomes; `writer.rs` reaches
532 / 549 regions, 23 / 24 functions, 312 / 315 lines, and 63 / 64 branch
outcomes. Ordinary tests reach 204 / 218 serializer regions and all 64 writer
branch outcomes. These measurements remain below the project's 100% target.

Checked-in summaries are [fuzz coverage](validation/fuzz-coverage-b181960.txt)
and [ordinary coverage](validation/test-coverage-b181960.txt). Raw local artifacts
are `/private/tmp/yaxml-coverage-serialize-fuzz` and
`/private/tmp/yaxml-coverage-serialize-tests`; they retain profiles, merged data,
JSON exports, reports, compiler/reporter versions, revision, and production
source hashes. The deterministic fuzz smoke passed 20 functions. This is not a
sustained coverage-guided campaign.

## Oracle and performance evidence

An exploratory system libxml2 2.15.3 run compared every corpus input and every
successful serialized output: 12 semantic agreements, three rejection
agreements, one documented DTD-policy skip, and no other disposition. This is
not the required pinned 2.15.4 result.

The release serialization runner completed all 60 default combinations of five
tree shapes, three sibling counts, borrowed/owned storage, and counting/8 KiB
buffered sinks. Trees are built outside the clock; each iteration includes a
fresh Writer, complete serialization, final flush, sink verification, and
destruction. Separate depth-4,096, 256-attribute/4 KiB-URI, and 128 KiB lexical
payload selectors completed. These single-machine yaxml measurements validate
the runner and workload limits; they are not a statistical baseline or a
libxml2 comparison. The raw logs are
`/private/tmp/yaxml-serialize-benchmark-release.txt` and
`/private/tmp/yaxml-serialize-oracle-exploratory.txt`.

## Remaining gates

- Run the serialized-output oracle against the pinned libxml2 2.15.4 build.
- Build a matched libxml2 `xmlSaveDoc` callback-sink benchmark and collect
  repeated timing, allocation, retained-memory, and peak-RSS series.
- Add deterministic allocation-failure injection and close the reachable source
  gaps before claiming the 100% execution target.
- Run a sustained feedback-guided fuzz campaign and triage every unique result.
- Implement namespace repair, subtree output, formatting/save options, UTF-16
  output, and the remaining libxml2 writer/save compatibility surface.
