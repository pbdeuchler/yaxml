# Private DTD Reader process-integration design review

Plan: [private DTD Reader fuzzing](plans/dtd-reader-fuzz.md).
Implementation base: `dc81e6d4542fc04d89e6ced2c6eb767fea09f1dc`.
Tree: `3c2fbf201e45bdbafb00fabc6c2f2784ef45ffe8`.

Verdict: PASS, with no open Critical, Important or Minor design findings.
Implementation and real campaign evidence remain separate gates.

## Frozen design

The process matrix makes the captured Master32 `SourceInventory` the sole
source baseline from Cargo preparation through candidate execution, LLVM
consumption, commit and final publication. Cargo cannot accept an arbitrary
source slice or capture a replacement baseline. The two-phase preparation
boundary freezes phase-one roots and inputs before the inventory is captured,
then consumes both owners by value.

Raw, cumulative and pending LLVM profiles use bounded same-handle identities
with explicit storage transitions. Every target and LLVM result is separated
from interpretation by source and profile gates. The first Master31 mapping
audit and every Stable21 report remain candidate-local until the cumulative
profile commit and destination verification succeed.

Evidence uses one bounded append-only profile-gate ledger with an immutable
on-disk lifecycle. It is content-checked before commit, finalization and each
checked report publication. Report fields distinguish configured values,
observed results, committed audit state, clean rollback, incomplete rollback
and invalid evidence. Aggregate ledger, path, diagnostic, report and fallback
report sizes have checked hard limits.

## Review evidence

The accepted matrix SHA-256 is
`45d3871d5db888660beac14d5f8b55f2df29b44fa67bf918fe0f0b0851ebbcd0`.
Its repair report SHA-256 is
`885a0e20c28dc9e9d529d945ba02b4c88d3610c990fbdf32c0234942f3d3fcc0`.

Independent final conformance passed with zero findings; report SHA-256:
`8c9d5750ec08286ca360f0882422fb59475d55250f54f452ded10d6a348ec538`.
Independent final holistic review passed with zero findings; report SHA-256:
`dad8a0abfc1eae3e42aa1551e3345e8267be290828dfe0f3c20c7e4cab5993f2`.

The implementation scout SHA-256 is
`51c286ada0dd583ff09fed4260e95d8243f37a0e3249f081bfd4f96cea626e7f`.
It divides the implementation into the retained-inventory handoff, Cargo CLI
and preparation ownership, exact libtest execution, Master/Stable audit and
commit, and final evidence closure. Each slice requires its own tests and
review before integration.

This design approves implementation of the private campaign path. It does not
claim a complete frontend source union, crate-wide coverage closure, a
production fuzzing result or public DTD activation.
