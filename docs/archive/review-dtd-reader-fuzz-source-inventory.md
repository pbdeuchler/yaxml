# Private DTD Reader source-inventory review

Plan: [private DTD Reader fuzzing](plans/dtd-reader-fuzz.md).
Integrated commit: `dc81e6d4542fc04d89e6ced2c6eb767fea09f1dc`.
Tree: `3c2fbf201e45bdbafb00fabc6c2f2784ef45ffe8`.
Base: `e47633bc68b4efe6070664479a4f13ff97b9ed24`.

Verdict: PASS, with no open Critical, Important or Minor findings for the
source-inventory stage. Campaign-process integration remains a separate gate.

## Inventory and mapping boundary

The campaign now freezes the exact source inventories for Master32,
PublicReplayV2, StableReaderV1 and the Master libtest diagnostic profile. It
derives collision-safe `SrcRelativeV1` row names, applies one MiB per-file and
eight MiB aggregate content limits to same-handle identities, and rejects
missing, extra, symlinked, oversized or ambiguously mapped inputs before the
coverage process can consume them.

Coverage rows contain exactly eight count fields. The implementation checks
their per-file denominators, preserves covered-count monotonicity and rechecks
the complete source inventory immediately before successful report
publication. The direct campaign migration and source-drift handoff gates use
the same frozen contract.

## Verification and limits

The final real-tool evidence compiled and self-tested its gate before running
the targets, gated all three raw profiles immediately before merge, and used
fresh gates before all six report and export consumers. Fifteen receipts and
nine before/after equalities cover empty, exact, growth, truncation and
substitution boundaries. The exact row gates are 4, 16, 31 and 21, including
all-eight-field equality for the Stable and Master profiles.

Independent final conformance and holistic reviews both passed with zero
findings. Their report SHA-256 values are
`bcecb960b86d8ff14aad04909f2e16c0723306067e3ea4ef2afe1b14aaa57518`
and
`b3ddadf303d3c13a64d4d45c2f0d77b13f30f4ef97beb665365d8502e269b25b`.
The independent integration review also passed with zero findings; its report
SHA-256 is
`98aa2724a1342daf3f27013c02608b7310237d3ba239cfeebd15153d2b4b53d7`.

The complete debug and release all-target suites pass at the integrated tree.
The library suite contains 802 passing tests and one ignored external-oracle
test; the fuzz-campaign suite contains 143 passing tests and one ignored
external-oracle test. Strict Clippy, formatting, diff and clean-worktree checks
pass. The final integration report SHA-256 is
`57b26fa20afefde275bd72afb3dc12c5c70dbdd403ca1b57c66633ad32e2ea2d`.

The next stage must make Master32 the sole immutable source baseline across
Cargo preparation, campaign execution, LLVM consumption, commit and final
publication checks. It must remove the weaker second source rebaseline before
the private DTD target is wired into the campaign CLI.
