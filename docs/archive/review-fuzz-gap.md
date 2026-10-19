# Focused fuzz-gap integration review

Verdict: **PASS for the bounded harness change**. This review covers the diff
from `9e4b3a6` through `1821a98`, followed by documentation corrections in the
same integration branch. Production sources and dependency manifests are
unchanged. It does not certify 100% coverage or a complete security posture.

The orchestrator reviewed every added assertion and generated-input bound.
Finish cancellation uses at most 16 input-derived ASCII bytes, independently
checks the event payload/range and exact cancellation offset, and rejects
callbacks after terminal failure. Encoding rejection checks a fixed bounded
BOM-less document in the selected byte order across push, buffered and owned
input. The new whitespace case pins the source offset rather than comparing
only the error kind. Negative oracle tests reject deliberately shifted offsets.
No production behavior, dependency or unsafe code was added.

The author separately audited the integrated committed source/harness hashes,
re-merged the saved raw profile and regenerated its report/export. The results
are byte-identical to the retained artifacts. All 14 production source files and
27 harness files match the measured commit. Raw profiles from different source
revisions were not combined. Collapsed source spans remain explicitly separate
from LLVM's official region denominator.

Independently, the orchestrator changed `validate_encoding` to accept a missing
required encoding label. The new BOM-less test failed with `pull accepted
invalid XML`; the change was restored. Retained log:
`/private/tmp/yaxml-next-review-mutant-declaration.txt`. The author's five
additional deliberate faults and exact expected failures are recorded in the
[triage report](validation-fuzz-gap-triage.md).

After restoration, the integrated tree passed 346 all-target tests with seven
ignored subprocess helpers, ten documentation tests, strict all-target Clippy
and formatting. Logs are `/private/tmp/yaxml-next-review-tests.txt`,
`/private/tmp/yaxml-next-review-doc-tests.txt` and
`/private/tmp/yaxml-next-review-clippy.txt`. A final source diff against
`9e4b3a6` was empty. The reported ordinary-test count includes three custom
harness executions omitted by the earlier test-list count.

The accompanying construction document is a proposal with explicit storage,
aggregate work and public compatibility gates. It is not an implemented API.
The accompanying UTF-16 batching report rejects its prototype and preserves
the observed regressions. Neither experiment's production code enters this
integration.

Follow-up artifact review also recomputed the rejected attribute-block
experiment's two edited-state matrices: each has 868 retained process rows
covering 31 samples for every case/migration/mode. Group inventory, unchanged
observer checksums and logical counts, physical/live/obsolete accounting,
descriptor counts, medians and MADs agree with the report. The sparse edited
regressions reproduce at 6.59% and 6.38%. Every entry in the retained
`SHA256SUMS` verifies when run from the prototype repository. Only the plan and
rejection report are integrated; prototype implementation and tests stay on the
experiment branch. The audit is retained in
`/private/tmp/yaxml-attribute-blocks-root-audit.txt` and
`/private/tmp/yaxml-attribute-blocks-root-manifest-check.txt`.

A second report audit reproduced all three UTF-16 screens' semantic pair
fields, final summary medians and hashes. It identified fixed build order per
case and missing retained source-to-binary manifests; the report now states
both limitations explicitly. Its rejection decision is unchanged.
