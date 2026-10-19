# Private DTD Reader Cargo artifact-selector review

Plan: [private DTD Reader fuzzing](plans/dtd-reader-fuzz.md).
Integrated commit: `8e29a9bea2488348d61dbf0d483ee1fe0c5ca2a4`.
Tree: `b9ed6aa77c7638b5ecc81a00f73a596e10d3c26b`.
Base: `94b38330b23d43fa872675a40fb819c21b55e64a`.

Verdict: PASS, with no open Critical, Important or Minor findings for the
bounded JSONL and semantic-selector slice. Filesystem containment, CLI and
campaign process integration remain separate gates.

## Parser and selector boundary

The std-only parser accepts complete Cargo JSON lines under explicit limits for
stream bytes, line bytes, records, nesting depth, nodes, object members, array
elements, keys and decoded strings. It implements JSON number grammar, UTF-8,
all standard escapes and valid surrogate pairs. Malformed UTF-8, controls,
escapes, numbers, trailing bytes, lone surrogates and every duplicate object key
fail before selection.

Selection requires the separately recorded build process status to be success
and the final JSON record to be one successful `build-finished` message. It then
selects exactly one `compiler-artifact` with the exact package ID, library
target kind and crate type, target name and source path, edition and test/doc
metadata, `profile.test = true`, and a non-null string executable. Well-formed
unrelated package and target records are ignored. A malformed current-package
library-test candidate fails explicitly, and zero or multiple matches are
typed errors.

Independent fixtures cover key reordering, irrelevant fields, dependency and
wrong-target artifacts, UTF-8 paths and spaces, successful and failed build
completion, missing and wrong field types, zero and two matches, JSON grammar,
all parser limits and exact inclusive boundaries. Disposable changes proved
the suite detects package substring matching, duplicate-key acceptance, lone
surrogate acceptance, early selection before build completion, ignored
malformed candidates and a total-byte boundary error.

## Linear Unicode repair

Holistic review found that the first candidate revalidated the remaining input
suffix for every raw non-ASCII scalar. A 131,142-byte record took 2.375 seconds
versus 226 microseconds for equal-size ASCII. The repaired decoder validates
only the current two-, three- or four-byte scalar after one whole-line UTF-8
check. A deterministic regression reduces measured scalar-validation work from
100,696,064 bytes to exactly 24,576 for the same fixture. A separate boundary
probe parses exactly 1 MiB of raw Unicode and rejects the next oversized value
in 0.01 seconds while preserving exact offsets and decoded-byte accounting.

The final conformance review passed with report SHA-256
`22daa1e2f251c89eaf6221eb4bef3d5d864c5c1e5348e5fb19e2eba646b43d12`.
The final holistic review passed with report SHA-256
`6ec7cd28854d5c1e58ab8818405eb78ac4c589014113c5156d54c07c456838f2`.

The integrated example passes 45 tests in debug and release. Strict test and
example Clippy, formatting and diff checks pass. The parser remains test-only
until canonical-path containment, selected-executable identity and campaign
invocation are wired and reviewed.
