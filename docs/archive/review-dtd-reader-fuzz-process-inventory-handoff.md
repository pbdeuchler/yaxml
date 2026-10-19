# Private DTD Reader process inventory-handoff review

Plan: [private DTD Reader fuzzing](plans/dtd-reader-fuzz.md).
Integrated commit: `693389925453505ad5c649c66dfc1e76a654c961`.
Tree: `45eb8f856bde251eeda01c6f082a423b5d8f97a5`.
Base: `dc81e6d4542fc04d89e6ced2c6eb767fea09f1dc`.

Verdict: PASS, with no open Critical, Important or Minor findings for the
Cargo source-inventory handoff. Cargo campaign execution and coverage evidence
remain separate gates.

## Handoff boundary

Cargo preparation now consumes one non-cloneable Master32 `SourceInventory`
through a two-phase bootstrap. It cannot accept an arbitrary source list or
capture a replacement source baseline. Structured failures retain the bounded
inventory reason and path, and full inventory checks surround every preparation
command and JSON interpretation with fixed process-first failure precedence.

The bootstrap owns the fresh output decision and canonical parent. Phase two
rejects output or parent drift before scratch creation or process launch. During
publication, preparation freezes the exact directory immediately after it
creates it, checks that root before and after moving the original scratch
directory, and transfers the original directory key and owner marker to
`output/build` without recapture. Cleanup removes only identity-proven empty
directories and preserves substituted paths.

## Review evidence

Independent final conformance and holistic reviews both passed with zero
findings. Their report SHA-256 values are
`336261f6f45e2cb4b2801bd5ac6b83063cc6d614581b92a66382af6757675d6d`
and
`2a85d97ffbbd23df18fcfc852d0e6bfa63c75920efa7a73eff3e97b2e29bf304`.

The focused debug and release Cargo-preparation suites each pass 34 tests with
one pinned-tool fixture ignored. Full debug and release all-target validation
passed with two unchanged Unix-socket tests filtered because the managed
sandbox rejects socket creation. Strict Clippy, formatting, whitespace and
clean-worktree checks pass.

Mutation evidence covers source rebaselining, deleted command gates, stale
bootstrap output authorization, substituted publication roots and
byte/stamp-identical build-child substitution. Each mutation turned its named
test red before byte-exact restoration.

The final integration report SHA-256 is
`da32cf4bfac4e5679940d6110b7b125a9bcd081aab4089ee981b0957d70c22f5`.

The next slice adds the user-facing Cargo mode and transfers the complete
prepared owner into `Campaign` while preserving the Direct-mode golden. Exact
libtest execution, profile identity, Master/Stable audits and checked evidence
publication follow as separate reviewed slices.
