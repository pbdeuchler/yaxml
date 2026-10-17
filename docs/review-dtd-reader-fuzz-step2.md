# Private DTD Reader fuzz boundary-matrix review

Plan: [private DTD Reader fuzzing](plans/dtd-reader-fuzz.md).
Reviewed candidate: `8f21efb76368dd7d4d8692f586a732f0d2e0425a`.
Integrated commit: `9af4644a94cdb57cfc228feea1a66bd508130f28`.
Tree: `27190e5f21c85fd787244daecd0f2dddd580bddd`.
Base: `446613fc2261181423044dc91b057095ea6a6c41`.

Verdict: PASS, with no open Critical, Important or Minor findings. This is the
second private driver checkpoint. It establishes a bounded deterministic test
matrix and independent oracles; it does not establish measured LLVM coverage
or complete the feedback campaign.

## Boundary and oracle coverage

Control bytes four through seven now select a canonical witness, a literal
truncation point, one of 48 quota or precedence axes, and a lifecycle mode.
The implementation preserves the first checkpoint's four generated-document
families and covers all 256 selector-byte values plus short headers. Missing
control bytes have an explicit zero interpretation.

The canonical fixture bank checks ordered event values, typed DTD views, Cow
and raw storage classes, exact borrowed occurrences, QName prefix/local parts,
and complete physical or generated origins. Success, quota rejection and
truncation paths cross retention, consuming extraction and early-stop modes.
Receipts are written only after the selected operation completes, and retained
events are checked after later reads, terminal retries and Reader drop.

Owned evidence remains bounded to 16,384 bytes and 128 objects. Admission is
checked before cloning or moving. Dedicated cases cover a 1,024-byte namespace
URI across 80 generated names and a 65,536-byte normalized owned value. Direct
input remains capped at 1 MiB; the prior generated-source, logical-expansion,
declaration, operation and transcript bounds are unchanged.

The only source-module additions are narrow `#[cfg(test)]`, `pub(crate)` move
and inspection helpers. Release library compilation confirms that they do not
enter the normal library surface.

## Independent review

Two detached reviews independently passed the final tree. Between them they
introduced and detected every requested regression class: removal of each real
route; selector aliasing; omitted lifecycle completion on success, rejection
and truncation; value changes that preserve length; cloning before admission;
wrong equal-string occurrences; broad or cleared origins; wrong DTD raw
storage; inherited namespace occurrence mistakes; zeroed snapshots; omitted
namespace accounting; skipped empty invocation identity; and corrupted QName
prefix or local parts.

The final conformance review report has SHA-256
`8e82862340f9a4e05065ce10ff2325ec0e659ae6d28fcf1e7c0975f1c962186e`.
The final holistic report has SHA-256
`2456f2c486750aa80e1b933c09f5ebc79931ae7d540c39171fc3bdb3062d3e72`.

## Integrated verification

The integrated tree passes 1,240 tests with ten documented ignores in both
debug and release all-target runs. Thirteen doctests pass, including five
compile-fail tests. The exact ignored stdin entry passes with empty stderr.
Strict all-target Clippy, formatting, diff checks and the release non-test
library check also pass, and the integration worktree is clean.

The next checkpoint is the versioned libtest process protocol, exact Cargo
artifact selection, collision-safe source-row identities and bounded campaign
accounting. The frontend driver matrix, measured coverage closure, performance
gates and public DTD activation remain open.
