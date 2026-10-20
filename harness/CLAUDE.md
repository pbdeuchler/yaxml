# yaxml-harness agent notes

Benchmarks, fuzzing, libxml2 2.15.4 differentials and XMLTS conformance for yaxml (core repo: sibling `../yaxml`, its CLAUDE.md has the architecture). Canonical repo: bare `../yaxml-harness.git`, branch main; core worktrees `../yaxml-wt/<name>`.

## Coupling to core (keep in sync)
- Cargo dep `yaxml = { path = "../yaxml" }` (fuzz/: `../../yaxml`); bin `yaxmllint` built from `../yaxml/src/bin/yaxmllint/main.rs` (for `CARGO_BIN_EXE_yaxmllint` in tests/xsd_suite.rs). To test a core worktree, make `../yaxml` (relative to this checkout's real path) a symlink to it; `#[path]` resolves physically, so the harness checkout itself must not be a symlink.
- `#[path]` includes from core: `tests/support/fuzz/driver.rs` (fuzz targets, examples/fuzz_replay.rs), `tests/support/fuzz/text_script.rs` (tests/text_writer_oracle.rs), `tests/support/html_corpus.rs` (tests/html_oracle.rs). Fuzz logic lives in core, never here; seeds `../yaxml/tests/fixtures/fuzz-seeds/` (extra read-only corpus dir); fixed regressions in both `fuzz/regressions/<t>` and core `tests/fixtures/fuzz-regressions/<t>`.
- Core keeps its own ignored/env-gated differentials (dtd, valid, valid_stream, relaxng, resolver, xsd, xsd_random, schematron, c14n, catalog, xinclude, yaxmllint); `oracle.sh` runs core with `--include-ignored`.

## Layout
- lib.sh: shared locations (YAXML_ORACLE=../yaxml-oracle, YAXML_SUITES=../yaxml-suites, FUZZ_TOOLCHAIN, CARGO_TARGET_DIR=target, FUZZ_TARGET_DIR=fuzz/target, RESULTS=results), `helper NAME` (compiles tooling/libxml2_NAME.c against the oracle when stale -> target/helpers/NAME), `need_*` checks, `help_if` (prints the script's header comment).
- setup.sh: clone yaxml, fetch+sha256+build libxml2 (CMake flags = ../yaxml-oracle's: static, ICONV/ICU/ISO8859X/ZLIB/PYTHON off; schematron off, so the schematron differential is skipped unless YAXML_SCHEMATRON_HELPER is set), fetch XMLTS, generate xmlconf-catalog.tsv (2586 lines) with the xmlconf_catalog helper.
- oracle.sh: exports YAXML_XMLLINT, YAXML_LIBXML2_SRC/_SOURCE, YAXML_SUITES, YAXML_XMLCONF and every YAXML_*_HELPER; steps core / harness (`--include-ignored --skip child_`: oracle_process child stubs) / replay / structure / xmlconf; release profile with LTO off (CARGO_PROFILE_RELEASE_* env). Skips core `doubling_input_does_not_quadruple_time` (timing).
- bench.sh: parse_benchmark with the parse_bench helper; ratio = libxml2 median / yaxml median of (parse_ns+drop_ns)/iterations; Cargo.toml release profile matches core's (thin LTO, 1 CGU).
- Tests building examples (`*_cli.rs`, oracle_cli) run `cargo build --example` in this package; fixtures for them in tests/fixtures (fake_*.rs, create_oracle_expected.tsv); conformance baseline tests/fixtures/xmlconf-20130923-required.tsv (9,728 pairs, 1,962 IDs).
- fuzz/fuzz_targets/allocfail.rs: the only unsafe code here (`#[global_allocator]` failing one chosen allocation, `#![feature(alloc_error_hook)]`); logic in core tests/support/fuzz/allocfail.rs; env YAXML_ALLOCFAIL_{SWEEP,TRACE,MIN_SIZE}; regressions/allocfail replay only with SWEEP (the chosen index shifts with code).
- Core keeps namespace errors without failing (`errors()`); conformance_worker (XMLTS scores namespace well-formedness), fuzz_replay (xmllint `--strict-namespace`) and parse_bench (the C helper fails on any error) count a kept error as rejection.
- Corpus: committed as is (21.5 MB real bytes, 46k files <= 4 KiB, already edge-only cmin'd); don't commit smoke-run additions (`git clean fuzz/corpus`).

## Gate (every merge)
```
cargo fmt --check && cargo clippy --all-targets -q -- -D warnings && cargo test --all-targets -q
(cd fuzz && cargo +nightly-2026-10-18 fuzz build -O)
sh -n *.sh && ./oracle.sh   # every step ok; xmlconf: required_pass_pairs=9728 required_pass_regressions=0
```
Scripts: POSIX sh, `set -eu`, header comment = `--help`, clear `die` on missing prerequisites. CI: .github/workflows/ci.yml (oracle, fuzz-smoke 20 s/target, coverage) checks out yaxml beside this repo.
