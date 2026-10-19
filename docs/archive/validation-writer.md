# Event writer validation

This milestone adds a bounded UTF-8 event writer to the existing reader, DOM and
push parser. It follows the [declaration/oracle report](validation-oracle.md).
The [writer contract](../writer.md) records the supported behavior and remaining
serializer surface. This is another implementation step toward the replacement,
not a complete libxml2 writer or a production-security claim.

## Behavior and regression evidence

At `66deabb`, Rust 1.95.0 on `aarch64-apple-darwin` passes 110 all-target test
functions and two README doctests. Six subprocess helper entries remain ignored
by the ordinary test runner because their parent test launches them explicitly.
Formatting, strict all-target Clippy and whitespace checks pass. The only Cargo
dependency remains `memchr` 2.8.3; the crate still forbids unsafe code.

Five private writer tests check scratch growth, escaping and metadata guards.
Fifteen independent public tests cover exact output bytes, every event kind,
namespace scope/aliases, supplied name metadata, invalid declarations and raw
delimiters, normalization, generated logical values, all six resource ceilings,
partial writes, invalid counts, error causes, Interrupted retry, flush failures,
panic poisoning and explicit completion. Writer destruction and `into_inner`
perform no implicit I/O; a sink's own destructor retains its usual behavior.

The writer validates a complete generated token through the shared semantic
engine before calling its sink. Its explicit raw-delimiter and character checks
establish the lexical assumptions that the internal engine expects. This shares
grammar and namespace behavior at the cost of encoding and revalidating each
event; no serializer throughput or allocation advantage has been established.

The fuzz driver now exercises parsed-event round trips, independent generated
writer operations and scheduled output faults. Generated operations include
arbitrary qualified spellings, incoherent public fields, declaration and PI
injection, forbidden raw delimiters and independently varied policy bytes.
Completed output must reparse; known forbidden values must be rejected before
output; failures must poison later calls. The [target inventory and input format](../../fuzz/README.md)
distinguish these checks from ordinary tests and retained gaps.

Review found two fuzz-oracle weaknesses: removing raw PI delimiter rejection or
the local-name coherence check was caught by ordinary tests but survived the
initial fuzz smoke run. Explicit expected-rejection invariants were added for
forbidden raw delimiters and generated inconsistent names/namespaces. Reparse
alone cannot establish that a caller-created event was interpreted correctly.
The [mutation review](review-writer.md) repeats both formerly missed mutations
and verifies namespace coherence, partial positions and panic poisoning. All
five final mutations are caught; no findings remain open in this milestone.

Streaming and writer round-trip fuzz comparisons now advance borrowed readers
in lockstep, preserving typed values, namespaces and spans without cloning a long
shared namespace URI for each attribute/event. Owned snapshot helpers moved to
bounded ordinary test fixtures. A shared-URI regression exercises this path;
this is not a measured process-memory quota or a production allocation fix.

## Source coverage

Both measurements use the clean `66deabb` source/test snapshot, default features,
debug optimization and pinned nightly `2026-06-06`, with
`-C instrument-coverage -Z coverage-options=branch`. The compiler is Rust
1.98.0-nightly / LLVM 22.1.6. Local Apple LLVM 21.0.0 tools read the profiles;
matching local LLVM tools remain unavailable. The generic-instantiation summary
and compiler branch-denominator limitations in the [streaming report](validation-streaming.md)
still apply.

| Run | Regions | Functions | Lines | Instrumented branch outcomes |
| --- | --- | --- | --- | --- |
| Fuzz smoke only | 2,581 / 2,719 (94.92%) | 146 / 161 (90.68%) | 1,654 / 1,696 (97.52%) | 397 / 416 (95.43%) |
| Ordinary parser/DOM/writer tests only | 2,547 / 2,719 (93.67%) | 146 / 161 (90.68%) | 1,649 / 1,696 (97.23%) | 390 / 416 (93.75%) |

Writer-only fuzz coverage is 479 / 497 regions (96.38%), 18 / 19 functions
(94.74%), 283 / 286 lines (98.95%) and 61 / 62 instrumented branch outcomes
(98.39%). Allocation-error closures and defensive internal coherence guards
remain in the denominator. Caught sink panics have ordinary regression coverage
and are not injected by the fuzz target. These are open coverage items.

The checked-in [fuzz summary](validation/fuzz-coverage-66deabb.txt) and
[ordinary-test summary](validation/test-coverage-66deabb.txt) exclude only
dependency/toolchain and test/fuzz/example files; no production region is
excluded. Complete JSON exports retain those files. Ignored local
`coverage/fuzz-66deabb/` and `coverage/tests-66deabb/` preserve raw/merged profiles,
compiler/tool versions, artifacts, source/corpus hashes, artifact checksums and
test logs. The fuzz run has 16 smoke functions, zero unexpected failures and
2.05 seconds elapsed. Individual testcase counts are still unrecorded. The
ordinary run contains 78 library/conformance/parser/DOM/streaming/I/O/writer
test functions and excludes fuzz smoke, oracle tooling, examples and benchmarks.

The 100% execution target, sustained coverage-guided campaigns, allocation-fault
injection, comparative performance, UTF-16 and broader compatibility remain
open. Earlier exploratory libxml2 2.15.3 results and the unexecuted pinned
2.15.4 CI gate are unchanged; this milestone establishes no new pinned-oracle
pass.
