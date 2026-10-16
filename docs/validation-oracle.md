# Declaration and oracle validation

This follows the [streaming milestone](validation-streaming.md). It adds
spec-derived conformance regressions and an independent, bounded semantic
comparison tool. Neither a full conformance pass nor libxml2 equivalence is
established by this milestone.

The reviewed integration passes 88 all-target test functions and one README
doctest on Rust 1.95.0, `aarch64-apple-darwin`. Formatting and strict all-target
Clippy pass. The dependency graph still contains only `memchr` 2.8.3; no unsafe
code or Cargo dependency was added.

## Parser behavior and conformance

The [conformance suite](conformance.md) found two declaration defects. Forward
`1.x` declarations were incorrectly rejected; they now use XML 1.0 processing
rules and preserve source version spelling. Malformed encoding names were
incorrectly classified as unsupported; syntax is now checked first. The shared
semantic engine applies both fixes to slice, DOM and push parsing.

Seven new conformance tests check explicit grammar acceptance and logical values,
rather than only frontend agreement. An [independent declaration review](review-declarations.md)
found no issues and verified three deliberately broken predicates fail their
expected tests. New declaration, malformed-encoding and infoset corpus seeds
also run through the deterministic fuzz driver.

## Independent comparison and process boundary

The [oracle protocol](oracle.md) evaluates generated XPath against original bytes
using external libxml2. Its full values never pass through yaxml's parser. It
compares normalized node order/kinds/depth, names and namespace identities,
attribute order/values, text, comments and PIs. CDATA/text grouping and the omitted
lexical/declaration/namespace-placement details are explicit protocol choices.

At revision `6713cbf`, the exploratory local libxml2 **2.15.3** run reports six
acceptance/infoset agreements, two rejection agreements, one unsupported-DTD skip,
zero acceptance-only cases, zero inconclusive results and zero disagreements.
The [complete output](validation/oracle-21503-6713cbf.txt) records the executable
and build features. Strict mode separately refused this binary because the
required version is **2.15.4**. The target has not changed to the installed version.

The external runner checks finite deadlines and combined output bounds, avoids
pipe joins, preserves nonzero status for classification and explicitly empties
catalog variables. Real subprocess tests check input EOF, output limits, timeout,
spawn failures, nonzero exits and cleanup. Its six ignored helper entries are
intentionally launched by the parent test; they are not six untested cases.
Polling disk overshoot, direct-child termination and best-effort cleanup remain
documented limitations of this trusted-tool boundary.

A command regression builds and executes the actual replay example against
controlled safe Rust oracle executables. Nine cases verify real exit status,
version policy and exact summary counts, including inconclusive results and both
directions of acceptance disagreement. These doubles test the CLI boundary;
they are not evidence of libxml2 semantics. The test requires Cargo and rustc,
builds offline into a private temporary directory, and verifies cleanup.

Review initially found that dropping the main inconclusive-result failure gate
survived helper tests. The command regression now exercises that boundary. CI
compiler/revision/corpus metadata is collected before replay so a failed
comparison cannot skip those artifacts.

CI is configured to verify GNOME's source-archive checksum and build static
2.15.4 with recorded CMake features. Workflow YAML and all seven shell steps
parse locally. Neither this build nor hosted CI has executed here, so a pinned
semantic result remains an open gate.

## Source coverage

These measurements describe library code at `6713cbf`; oracle tooling has its own
tests and is outside this denominator. Raw summaries exclude dependency/toolchain code and
test/fuzz/example tooling, with no production regions excluded:

| Run | Regions | Functions | Lines | Instrumented branch outcomes |
| --- | --- | --- | --- | --- |
| Fuzz smoke only | 2,086 / 2,205 (94.60%) | 126 / 140 (90.00%) | 1,361 / 1,400 (97.21%) | 336 / 354 (94.92%) |
| Ordinary parser/DOM tests only | 2,069 / 2,205 (93.83%) | 126 / 140 (90.00%) | 1,356 / 1,400 (96.86%) | 327 / 354 (92.37%) |

The [fuzz summary](validation/fuzz-coverage-6713cbf.txt) and
[ordinary-test summary](validation/test-coverage-6713cbf.txt) are checked in.
Ignored local `coverage/fuzz-6713cbf/` and `coverage/tests-6713cbf/` retain raw and
merged profiles, complete exports, compiler artifacts, source/corpus hashes and
test output. The fuzz run has 14 smoke test functions, zero unexpected failures
and 1.41 seconds elapsed; individual testcase counts remain unrecorded. The
ordinary run has 58 unit/conformance/reader/DOM/streaming/I/O/callback tests; it
excludes fuzz smoke, oracle tooling, examples and benchmarks.

The compiler is nightly `2026-06-06`, Rust 1.98.0-nightly/LLVM 22.1.6, with
`-C instrument-coverage -Z coverage-options=branch`, default features and debug
optimization. Local Apple LLVM 21.0.0 tools read the profiles; matching local
tools remain unavailable. The generic-instantiation summary behavior and
instrumented-branch limitations from the streaming report still apply.
Allocation-failure closures and internal invariant guards remain in the raw
denominator. This short deterministic campaign does not close the 100% coverage
or sustained coverage-guided fuzzing goals.
