# Owned DOM validation

This milestone implements immutable owned UTF-8 storage after the
[event writer](validation-writer.md). The [plan](plans/owned-dom.md) and
[owned DOM contract](owned-dom.md) describe its scope. UTF-16, mutable trees,
binding registries and allocator parameters remain separate work.

## Functional evidence

The integrated implementation passes 128 all-target test functions and three
README doctests on Rust 1.95.0, `aarch64-apple-darwin`. The six ignored helper
entries remain deliberately launched by the real subprocess test. Formatting,
strict all-target Clippy and whitespace checks pass. The standalone owned-DOM
guide example was also compiled and executed. Cargo still has only `memchr`
2.8.3, and yaxml still forbids unsafe code.

Eight private tests verify empty copies, full replacement-capacity accounting,
budget clamping, geometric growth, stable pool ranges, Unicode byte splits,
borrowed views, shared parser accounting and topology links. Nine independent
public tests cover every node kind, declarations, namespace/default/predefined
and normalized URI views, exact ranges/navigation, input destruction, document
moves and cross-thread use, Unicode growth, malformed/unsupported input, all six
limits, aggregate budgets, document-local IDs and iterative deep/wide destruction.
An 8,192-level tree parses and drops on a 128 KiB thread stack.

Owned/borrowed trees are compared completely across accepted corpus fixtures.
Writer comparisons intentionally use explicit start/end pairs for empty elements:
the DOM preserves topology and logical values, not empty-tag lexical spelling.
Independent value goldens supplement comparisons through the shared parser and
topology helper.

The owning pool and parser use one normalization/storage counter. Exact tiny
construction minima include 1 byte for `<r/>`, 3 for `<r>a</r>`, 8 for
`<r>&amp;</r>` and 10 for `<r a='&#9;'/>`. These count each full replacement pool
capacity, even after releasing earlier buffers. They are also executable fuzz
oracles, so undercounting cannot evade detection merely by accepting more input.

The arbitrary-input fuzz path destroys a temporary source before retrieving
views, compares complete tree data and topology, and allows explicit earlier
OwnedBytes exhaustion. A conservative bound separately requires owned success
for accepted default-profile inputs up to 1024 bytes. Generated Unicode-prefix
and empty-value fixtures exercise byte boundaries and ranges across pool growth.
These checks and the new public APIs are mapped in the [target inventory](../fuzz/README.md).

The [independent mutation review](review-owned.md) found no open issues. It
deliberately undercounted pool capacity, used character counts for Unicode byte
splits, truncated element source spans and made the default constructor reject
everything. All four faults fail ordinary and fuzz tests; the restored suite,
doctests, formatting and strict Clippy pass with a clean review worktree.

## Source coverage

The fuzz run is pinned to clean `ee7b645`. The ordinary run uses clean `47f78df`;
production source is identical, and the later commit adds only a fuzz fixture.
Both use default features and debug optimization on nightly `2026-06-06`, Rust
1.98.0-nightly / LLVM 22.1.6, with
`-C instrument-coverage -Z coverage-options=branch`. Local Apple LLVM 21.0.0 reads
the profiles; matching local tools remain unavailable. The generic-instantiation
summary behavior and branch-denominator limits in the
[streaming report](validation-streaming.md) still apply.

| Run | Regions | Functions | Lines | Instrumented branch outcomes |
| --- | --- | --- | --- | --- |
| Fuzz smoke only | 3,012 / 3,176 (94.84%) | 191 / 211 (90.52%) | 1,938 / 1,987 (97.53%) | 408 / 428 (95.33%) |
| Ordinary parser/DOM/writer tests only | 2,972 / 3,176 (93.58%) | 191 / 211 (90.52%) | 1,933 / 1,987 (97.28%) | 401 / 428 (93.69%) |

Owned-storage source alone reaches 407 / 435 regions (93.56%), 40 / 45 functions
(88.89%), 264 / 271 lines (97.42%) and 11 / 12 instrumented branch outcomes
(91.67%) under fuzz smoke. Allocation-error closures, overflow handling and
defensive builder guards remain in the denominator. The new source expands the
coverage denominator; neither the total nor subsystem reaches 100%.

The checked-in [fuzz summary](validation/fuzz-coverage-ee7b645.txt) and
[ordinary summary](validation/test-coverage-47f78df.txt) exclude only dependency,
toolchain and test/fuzz/example files, with no production regions excluded.
Ignored local `coverage/fuzz-ee7b645/` and `coverage/tests-47f78df/` retain raw and
merged profiles, complete JSON exports, instrumented executables, compiler/tool
versions, source/corpus hashes, artifact checksums and logs. Fuzz smoke ran 17
functions with zero unexpected failures in 2.36 seconds; individual testcase
counts remain unrecorded. The ordinary coverage run contains 95 library,
conformance, reader, DOM, streaming, I/O and writer test functions; it excludes
fuzz smoke, oracle tools, examples and benchmarks.

## Performance and remaining evidence

The existing runner now includes an `owned` parse/destruction mode alongside
borrowing DOM, events and push input for all five fixtures. A 100-iteration
release smoke run completed. Raw before/after output is retained in ignored
`coverage/owned-benchmark-smoke/`. These fixed batches are uncalibrated, have no
alternating trial/MAD analysis and do not compare an equivalent libxml2 workload;
they establish runner operation, not superiority or a regression conclusion.

The pool baseline copies logical namespace URIs and bounds that work through its
aggregate string budget. It does not introduce speculative interning or custom
allocators. Metadata capacities and allocator overhead remain outside the string
budget. Input-size series, retained-memory measurements, matched performance,
allocation-fault injection, sustained coverage-guided fuzzing and complete
execution coverage remain open. Pinned libxml2 2.15.4 execution has not been
established by this milestone.
