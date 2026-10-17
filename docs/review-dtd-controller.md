# Private DTD controller and provenance review

Private freeze: `f43f90b0b502fd1e913ee9128d116f9bd5ad8b67`.
Source tree: `1d5896acfc99899c9be0c7e64d5fcd2a29ce3358`.
Worktree: `/private/tmp/yaxml-dtd-integration`, branch
`integrate/dtd-prototype`. Main retains its production source from `f2bd043`.
This extends the [semantic/frame review](review-dtd-semantics.md); none of these
private modules or representation changes is being activated on main.

Verdict: suitable for the next private attribute/content integration stage.
Public DTD acceptance remains blocked on the complete runtime path, source-complete
fuzzing, conformance, serialization, layout and performance evidence.

## Controller

The [incremental controller](plans/dtd-controller.md) joins the reviewed scanner,
unit grammar, catalog and entity frames. It emits ordered logical Begin, Item
and End deliveries, retaining one owned unit. Internal PE declarations appear
once at the expansion position; the physical reference is not also emitted.
Generated declarations can mutate the catalog and introduce another PE without
invalidating the active source. Two scratch buffers are bounded independently
of expansion depth. No complete expanded DTD/document or occurrence history is
retained, and no external resource is opened.

Physical progress pauses during generated input. UTF-16 mapping applies only to
physical decoded bytes; replacement-local offsets and outer physical reference
contexts remain separate. A no-subset header withholds its real closing `>` for
End rather than fabricating a zero span. Body bytes remain untouched. Each PE
must end between complete units; generated subset delimiters, nested DOCTYPEs
and context-invalid declarations fail before item publication.

Root review corrected cumulative scanner work across fragment resets, global
versus component work-error classification, replacement diagnostics on frame
quota failure, and allocation order for the mirrored active-ID stack. The ID
capacity is reserved before child-frame/parent-cursor publication. The catalog
also now reports unavailable values before duplicate-binding suppression:
discarding a duplicate cannot conceal a dependency failure from the controller.
These changes have separate author and independent regressions.

The private controller currently copies physical units as well as generated
ones. The eventual Reader adapter must retain direct UTF-8 input borrows without
reparsing declarations or borrowing its own scanner buffer. This lifetime and
copy-accounting integration is still open; no zero-copy DTD claim follows here.

## Origins and parsed text

The [provenance implementation](plans/dtd-provenance-text-joins.md) shares one
Input/Replacement/Mixed representation between frames and SourceOrigin. Numeric
declaration/invocation IDs remain private. Element closure permits a body gap
only within the same exact source occurrence; Text joining requires adjacency.
Different occurrences, gaps and source changes yield Mixed with an explicitly
inexact physical causal cover.

Both parse builders merge adjacent Text without creating another node or
changing links. CDATA, elements, comments and PIs remain barriers. Empty Text
is ignored and cannot conceal a source gap. Borrowed text promotes once, then
grows geometrically under the shared budget. Owned text extends the pool tail
using the existing reservation path. Quota/allocation failure preserves the
previous value, origin and topology. Editor and construction operations retain
separate adjacent node identities and absent constructed provenance.

The independent topology tests inject logical events into the real builders;
they do not claim the public parser already schedules GE content. A safe
capacity-overflow request exercises the String reservation error mapping.
Actual allocator OOM and complete parse-builder allocation failpoints remain
unforced.

| Private measured layout | Size/alignment, bytes |
| --- | --- |
| SourceOrigin / shared origin representation | 56 / 8 |
| Option<SourceOrigin> | 56 / 8 |
| Event | 88 / 8 |
| Borrowing tree record | 208 / 8 |
| Owned tree record | 176 / 8 |
| Reader | 392 / 8 |
| PushParser | 656 / 8 |

The previous origin was 16 bytes and owned record 144 bytes. This is a material
retained-memory cost, accepted only as a private correctness checkpoint. No
packed offset restriction, discarded identity or history arena hides that cost.
Public activation still needs measurement and a performance decision.

## Phase-aware output

The [physical-fragment encoder](plans/dtd-output.md) walks validated literal
metadata and CR positions monotonically, without allocating another raw unit.
Physical spelling stays intact. In replacement-origin EntityValue it encodes
literal CR as a numeric reference; in AttValue it emits a space. This preserves
the distinct numeric-reference phase and the two-space result of replacement
CRLF. Identifier CR has an explicit unrepresentable outcome because those
literal grammars lack a character-reference escape. Comments/PIs retain Writer's
existing documented reparse-normalization policy.

This is a private field encoder, not Writer support. A future Writer must retain
the literal phase, buffer and validate the whole event before sink publication,
implement Begin/End state, and account for output/quota/I/O failures. Literal
bytes and fresh-catalog round trips both have independent expected values.

## Verification

Fresh root gates on this exact clean freeze passed 764 tests with nine ignored
across 44 ordinary result groups, including 326 library tests, plus all 13
doctests. Strict all-target Clippy, formatting and diff checks passed. The
existing 64 fuzz-smoke tests and benchmark smoke binaries are included; they
do not constitute new DTD fuzz coverage or comparative timing evidence.
Runner `/private/tmp/yaxml-dtd-root-controller-final-checks.sh` verified the head
and clean status before and after. Handle `90955` is terminal with exit zero;
logs use `/private/tmp/yaxml-dtd-root-controller-final-*.log`. Toolchain:
Rust/Cargo 1.95.0, LLVM 22.1.2, aarch64-apple-darwin.

Independent additions include 17 controller, 20 origin/DOM and seven output
groups. Their oracle catalogs were frozen before implementation adaptation;
author tests remain separate. Tests cover every transport split in UTF-8 and
UTF-16LE/BE, literal ranges and body suffixes, normalized values, unused versus
included dependencies, exact limits, sticky failures, invocation identity,
text topology, ownership, pool/Cow growth and editing controls.

Deliberate runtime faults cover held delimiters, family gates, aggregate work,
reference anchors, hidden duplicate dependencies, wrong literal phases,
cross-occurrence joins, closure versus adjacency, disabled coalescing, omitted
quota charges, quadratic growth and lossy output. Each counted fault followed
a passing clean baseline, failed a runtime test, and was restored. Compilation
failure was not counted as detection.

Evidence includes root `/private/tmp/yaxml-dtd-root-controller-faults.tsv`,
`...-observability-faults.tsv`, `...-output-faults.tsv`; independent
`/private/tmp/yaxml-dtd-provenance-spec-report.txt`; and the separate commits
`d7a4a7c`, `9dd5271` and `ffae9e5`. The root verified all 310 tracked private
files byte-for-byte against their Git blobs; the source manifest is
`/private/tmp/yaxml-dtd-root-controller-source.tsv`.

The next implementation stages are start-tag preparation/defaults before
namespaces, GE content scheduling and lifetime-correct Reader/Stream adapters.
Public events, both DOM DTD catalogs and Writer must join that coherent path
before acceptance changes. Existing main coverage and XMLTS measurements do not
measure these private modules. Full libxml2 compatibility, 100% fuzz coverage,
allocator support and performance superiority remain open.
