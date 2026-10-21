# Working process

Adopted 2026-10-18, replacing the earlier multi-gate evidence process.

## Why it changed

By 2026-10-18 the repository held about 21k lines of library code, 34k lines
of bespoke campaign/process tooling and 57k lines of tests. The 13k-line DTD
stack had never been compiled into the public library: it was `#[cfg(test)]`
code behind many review and evidence gates. Process work had displaced product
work.

## Rules

1. **One slice, one review.** Each change is implemented on a branch, then
   the orchestrator reviews it for correctness, simplicity and tests and merges
   it. Separate conformance and holistic review reports, archive checksums and
   sealed evidence bundles are no longer required.
2. **Ship behavior.** Private code paths exist only briefly. A feature is
   activated publicly behind a well-specified API as soon as its tests pass.
3. **Standard fuzzing.** Coverage-guided fuzzing uses cargo-fuzz (libFuzzer)
   in the separate `fuzz/` package. Coverage is measured with `cargo fuzz
   coverage` and llvm-cov. Fuzz-only dependencies never enter the library's
   dependency graph, which stays memchr-only.
4. **Measured claims only.** Compatibility, coverage and performance numbers
   come from reproducible commands. Docs state gaps plainly, without ceremony.
5. **Tests.** Every new function gets happy- and sad-path tests. Table-driven
   tests are preferred. Differential checks run against the pinned libxml2
   2.15.4 oracle where behavior is observable.

## Locations

- Canonical repository: bare mirror `../yaxml.git` beside the workspace, with
  worktrees in `../yaxml-wt/`. The visible workspace directory has no `.git`.
- Pinned libxml2 2.15.4 oracle build: `../yaxml-oracle/build/xmllint`.
- Benchmarks, libxml2 differentials and the XMLTS run: `harness/` (its own
  Cargo workspace); fuzzing: `fuzz/` (its own cargo-fuzz workspace). Neither
  is built by plain `cargo test`. The root scripts drive them: `./setup.sh`
  fetches the oracle and suite, `./oracle.sh` runs every differential,
  `./bench.sh`, `./fuzz.sh` and `./coverage.sh` the rest.
- Backups of the pre-reset history and evidence: `../yaxml-backup/`.

## Toolchains

- Library: MSRV and `rust-toolchain.toml` are Rust 1.97.0 (CI matrix: 1.97.0 and
  stable). Reason: rustc 1.95 and 1.96 miscompile a safe struct-update pattern
  in release builds ([rust-lang/rust#158206](https://github.com/rust-lang/rust/issues/158206),
  MIR pass SimplifyComparisonIntegral, fixed by #158214 in 1.97).
  `tooling/simplify_comparison_scan.py` stays as the regression guard.
- Fuzz and coverage: `nightly-2026-10-18` (`fuzz/rust-toolchain.toml`, so plain
  `cargo fuzz` inside `fuzz/` works; from the root use `cargo +nightly-2026-10-18`).
  The earlier pin, `nightly-2026-06-06`, predates the fix. Campaign results in
  docs that name it were measured on it and are left as recorded.
