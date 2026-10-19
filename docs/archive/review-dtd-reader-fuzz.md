# Private stable-input DTD Reader fuzz-driver review

Plan: [private DTD Reader fuzzing](plans/dtd-reader-fuzz.md).
Reviewed implementation: `88afca43aae5cd0aa4389aa824acee11ac98f6d3`.
Tree: `c79a892637c02487f632d5bab0b5a505a96a7ac8`.
Base: `fc364ed9706e3f8ceccba034bae375bbaded7e73`.

Verdict: PASS, with no open Critical, Important or Minor findings. This is the
first private driver checkpoint, not a completed fuzz campaign or a coverage
claim.

## Driver boundary

The driver calls the private `DtdReader` directly from a test-only sibling
module. Its only observability additions are `#[cfg(test)]` snapshot and raw
source accessors; a release library check confirms that they do not enter the
normal library surface. Direct input is capped at 1 MiB. The exact ignored
libtest stdin entry reads one sentinel byte beyond that limit and rejects an
oversized case.

A deterministic 64-byte control format builds four bounded document families:
general entities, namespace/default `ATTLIST`, parameter-entity-created
declarations, and XML declaration/misc content. The largest selector uses four
declarations and eight nested references. Generated documents are at most 4 KiB,
transcripts at most 128 events and logical expansion at most 16 KiB; the largest
constructed case is 16,328 bytes.

The oracle independently builds ordered events, values, physical and
replacement origins, and the precise source occurrence for every borrowed
field. It distinguishes repeated equal and empty values using pointer-derived
ranges. It retains events across later reads and reader drop, checks generated
values after source drop, verifies borrowed/owned DTD raw storage, and exercises
sticky errors, repeated EOF and snapshot counters. A bounded insert/delete/
replace smoke pass covers the semantic corpus.

## Review repairs and verification

Initial review found weak per-field occurrence discrimination and incomplete
structured-family reachability. The final implementation gives each modeled
field an exact independently derived range and makes all four families directly
selectable. Focused negative tests distinguish repeated equal and zero-length
occurrences. The maximum nested selector and all frozen caps have explicit
tests.

The reviewed branch passes 16 focused tests with one ignored stdin entry, 686
library tests in debug and release, and all 17 driver tests when the ignored
entry is included. Strict Clippy, formatting, diff checks and the release
non-test library check pass.

Selector-driven truncation, the 40 scalar quota axes, precedence collisions,
the exact campaign protocol, sustained coverage-guided execution and measured
LLVM coverage remain subsequent steps. Public DTD activation and any 100%
coverage claim remain deferred until those gates close.
