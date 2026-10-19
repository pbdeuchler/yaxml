# Node string-value chunks

Implementation snapshot: `51944f1` (the methods were introduced in `c04a409`).
`Node::text_chunks()` and `OwnedNode::text_chunks()` expose borrowed string-value
pieces without allocating. They reuse the existing iterative subtree traversal
and add no tree storage, dependency or unsafe code.

Documents and elements visit descendant Text/CDATA in document order, ignoring
comment/PI data. A leaf yields its own payload once, including an empty string.
The iterator preserves adjacent and empty chunks; `text()` retains its direct
leaf semantics. Collecting chunks into a `String` is a caller allocation, outside
the document's parsing/edit budgets. Entity, attribute, namespace and DTD nodes
remain outside the implemented node model.

## Independent libxml2 comparison

The pinned libxml2 2.15.4 Release build used elsewhere in this repository supplied
`xmllint` with XPath enabled. Its `xmlXPathCastNodeToString` calls
`xmlNodeGetContent` directly (`xpath.c:4403` in the pinned release). The comparison
used `--nonet --nocatalogs --strict-namespace --nowarning --xpath 'string(PATH)'`.
The harness removed exactly the final output newline from xmllint, preserving
embedded CR/LF bytes. Both programs had to exit successfully with empty stderr.

All 16 supported nodes in one mixed-content fixture were selected individually:
the synthetic document, elements including an empty element, Text, CDATA,
comments and PIs in the prolog, element and epilog. The fixture includes an
attribute whose value must be excluded, a predefined reference, a numeric CR,
Unicode and adjacent Text/CDATA. Borrowed and owned UTF-8 trees supplied 32
comparisons; owned UTF-16LE/BE trees with BOMs supplied another 32. Every joined
string matched exactly. The [64-row result table](validation/text-chunks-oracle-51944f1.tsv)
records each path, byte count and SHA-256. These are bounded cases, not complete
`xmlNodeGetContent` compatibility.

The safe Rust probe, Ruby harness, fixture bytes, compiler identity, executable
and SHA-256 manifest remain in `/private/tmp/yaxml-text-chunks-oracle`. The
xmllint executable SHA-256 was
`5592c3854dae2ed4739c90609215b6d983937ec9f1dbfb5d70967daa28b0698b`;
the result table SHA-256 is
`3d988b1b5151af9a9082597f8b160997e391fca2047c6403ae7c996791090654`.

## Regression and fault checks

Five public tests cover exact chunks for both views, per-node subtree boundaries,
empty values, normalization, UTF-16 ownership, linked order after edits, stale
removal, partial consumption, size hints, `count`, `last`, `nth`, repeated EOF and
8,192 nested elements on a 256 KiB thread stack. A compile-fail doctest requires
an owned iterator's shared borrow to prevent concurrent edits.

The fuzz driver compares whole-document chunks with independently captured
reader Text/CDATA events; owned parsed nodes with borrowing views; and edited
trees with the independent logical mutation model. Existing per-node projection
checks run first, retaining their specific failure diagnostics.

Eight temporary production faults were tested independently: admit descendant
comments, omit CDATA, repeat the selected leaf, or walk the whole document instead
of the selected subtree, once for each tree representation. Every fault failed
the public tests without a compilation failure. Original source was restored.
Logs use `/private/tmp/yaxml-text-chunks-mutant-{dom,owned}-` followed by
`include-comment`, `omit-cdata`, `repeat-leaf` or `leak-document` and `.txt`.

## Suite and fuzz-only coverage

On stable Rust/Cargo 1.95.0, the all-target suite passed 317 tests with seven
ignored subprocess helpers. The 32-test fuzz-smoke suite passed, as did ten
doctests, strict all-target Clippy, formatting and diff checks. The locked
dependency tree contains only `memchr 2.8.3`; crate and target roots forbid
unsafe code. A first full run exposed an expected-panic diagnostic ordering
change in the fuzz oracle; `51944f1` moved the new check after the existing
projection guards, and the full suite was rerun successfully.

A fresh instrumented fuzz-smoke executable built from `51944f1` ran all 32
fuzz-smoke tests, without ordinary test profiles. Nightly `2026-06-06` used
`-C instrument-coverage -Z coverage-options=branch` with matching LLVM 22.1.6
merge/report tools. All 14 production source paths were supplied explicitly;
`lib.rs` has no executable mapping. No source was excluded and merge/report
stderr was empty. This build has its own source hashes and profiles; earlier
production profiles were not merged into it.

| Metric | Reached / total | Coverage |
| --- | --- | --- |
| Regions | 4,855 / 5,082 | 95.53% |
| Functions | 315 / 340 | 92.65% |
| Lines | 3,269 / 3,346 | 97.70% |
| Instrumented branch outcomes | 657 / 692 | 94.94% |

The [complete per-file table](validation/fuzz-text-chunks-51944f1.txt) retains the
remaining gaps. These are execution measurements for the implemented crate,
not behavioral completeness or the requested 100% fuzz-coverage target.
No performance comparison is claimed by this change.

Profiles, source/harness/artifact SHA-256 manifests, compiler/tool versions,
per-source coverage and empty diagnostics remain in
`/private/tmp/yaxml-text-chunks-coverage-51944f1`; the exact build/run script is
`/private/tmp/yaxml-text-chunks-coverage-final.sh`. Stable verification logs use
`/private/tmp/yaxml-text-chunks-` with `all-targets-final.txt`,
`docs-final.txt`, `clippy-final.txt` and `projection-restored.txt` suffixes.

Independent holistic review of `fe2e8bd` returned PASS with no findings. The
reviewer reran all-target tests, doctests, lint/format checks, all 64 oracle
comparisons, three separate production faults and a fresh fuzz-only coverage
build. Its coverage table reproduced every count above; report SHA-256 was
`d7817b3b6797e0198768d282cdcaebbef9bc95d9d2d43f5ea93524a9fff27f0e`,
with raw artifacts in `/private/tmp/yaxml-text-chunks-coverage-review-fe2e8bd`.
