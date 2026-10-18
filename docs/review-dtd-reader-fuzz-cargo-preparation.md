# Private DTD Reader Cargo preparation review

Plan: [private DTD Reader fuzzing](plans/dtd-reader-fuzz.md).
Integrated commit: `e47633bc68b4efe6070664479a4f13ff97b9ed24`.
Tree: `60107a5e3bb9b77922fe3613bdc2d1523ca1519e`.
Base: `e3f7323129a4f713449f6cb249ed9c7fb827b500`.

Verdict: PASS, with no open Critical, Important or Minor findings for the Cargo
preparation stage. Source-inventory migration, campaign wiring and coverage
report execution remain separate gates.

## Preparation boundary

The example-internal preparation stage now runs the exact bounded six-command
Cargo/rustc/LLVM preflight and build sequence. It selects the test artifact from
the captured Cargo JSON bytes, retains create-new receipts for JSONL and stderr,
and freezes identities for the manifest, lockfile, Cargo, rustc and LLVM tools.
Full content gates run before the first process and before JSON selection;
metadata gates cover every intermediate handoff.

The selected executable remains owned through all success and failure paths.
Immediate and post-relocation content checks protect it from replacement,
growth and truncation. Root, input, receipt and selected-artifact drift are
reported as bounded paired diagnostics while the original process or I/O error
remains primary. Launch spelling is also checked through the child process's
observed `argv[0]`.

## Verification and limits

The integrated example passes 131 tests with one explicitly ignored real-tool
fixture in both debug and release. Across all example targets, 210 tests pass
and one fixture is ignored in each mode. The ignored fixture passed separately
with the pinned nightly Rust and LLVM 22 toolchain. Strict Clippy, formatting,
diff and clean-worktree checks pass.

Independent conformance passed 22 assertions and caught all 24 original plus
five focused mutations. Its final report SHA-256 is
`7b410c050ac3ac39122f23c8a943558eb4bb54b22eadaee6ac5bfff1bdce55ed`.
Independent holistic review caught seven deliberate regressions and passed with
report SHA-256
`a1dc59add3544c041e066c353ca4ec6934951a50515e103995a97aaccccff09b`.
The final integration report SHA-256 is
`2a6e18e6ff029f9712af57dc3b6b10e5201c975d61eeec9b986e8263fbe9ea56`.

This code remains under `cfg(test)` and does not yet expose a campaign CLI or
run the private DTD target. The next prerequisite is the exact 32-file source
inventory and collision-safe LLVM row mapping.
