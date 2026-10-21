# yaxml-harness

Benchmarks, fuzzing and libxml2 differential tests for
[yaxml](https://github.com/pbdeuchler/yaxml), a safe Rust libxml2 2.15.4
rewrite. yaxml itself needs none of this: `cargo test` there runs without
libxml2, test suites or a nightly toolchain. This repository holds everything
that does.

## Prerequisites

- A checkout of yaxml **beside** this one (`../yaxml`; a symlink works).
  `./setup.sh` clones it if it is missing.
- Rust via [rustup](https://rustup.rs); rust-toolchain.toml pins 1.97.0.
- For `oracle.sh` and `bench.sh`: git, curl, cmake and a C compiler.
- For `fuzz.sh` and `coverage.sh`: `rustup toolchain install nightly-2026-10-18
  --component llvm-tools-preview` and `cargo install cargo-fuzz --version
  0.13.2 --locked`. `coverage.sh` also needs [uv](https://docs.astral.sh/uv/).

Then run `./setup.sh` once. It downloads and checks (sha256) libxml2 2.15.4
and the W3C XML Test Suite 20130923, builds the static libxml2 oracle
(`../yaxml-oracle`, no iconv/ICU/zlib) and writes the suite catalog
(`../yaxml-suites`). Set `YAXML_ORACLE` or `YAXML_SUITES` to use other
locations; anything already present is reused.

## Commands

Every script takes `--help`.

| Command | What it does | Results |
| --- | --- | --- |
| `./oracle.sh` | All differential tests against libxml2 (yaxml's ignored ones included) and the conformance run, which fails on any regression of `tests/fixtures/xmlconf-20130923-required.tsv` | `results/oracle-<time>/`: one log per step, `xmlconf-results.tsv` |
| `./bench.sh [fixture...]` | Matched parse benchmark, yaxml vs libxml2, in five modes; prints libxml2/yaxml time ratios (above 1 favors yaxml) | table on stdout, raw samples in `results/bench-<time>.txt` |
| `./fuzz.sh [target [seconds]]` | Lists the fuzz targets, or fuzzes one (default 60 s) | new inputs in `fuzz/corpus/<target>/`, crashes in `fuzz/artifacts/<target>/` |
| `./coverage.sh [target...]` | Line/branch coverage of yaxml's `src/` from each target's corpus | `fuzz/coverage/report.txt`, `merged.lcov` |

Build output goes to `target/` (`CARGO_TARGET_DIR` overrides it) and
`fuzz/target/` (`FUZZ_TARGET_DIR`).

## Layout

- `tests/`: differential tests (`*_oracle.rs`, `xsd_suite.rs`) and tests of the
  tools below. Helpers they drive are `tooling/libxml2_*.c`; `oracle.sh`
  compiles them and passes their paths in `YAXML_*_HELPER` variables.
- `examples/`: the conformance runner (`xml_conformance`), benchmark drivers
  (`parse_benchmark`, `structure_benchmark`, `adversarial_scaling`,
  `reset_bench`), oracle programs and `fuzz_replay`.
- `benches/`: yaxml-only throughput benchmarks (`cargo bench`).
- `fuzz/`: the cargo-fuzz targets, corpus, regressions and open findings; see
  [fuzz/README.md](fuzz/README.md). The fuzz logic itself is in yaxml's
  `tests/support/fuzz/`, shared with yaxml's own tests.

## Adding a fuzz target

1. Write the checks as a `pub fn exercise_<name>(data: &[u8])` in yaxml's
   `tests/support/fuzz/` (a new module goes in `driver.rs` with a `#[path]`
   line), and call it from `tests/fuzz_smoke.rs` there with a few inputs.
2. Add `fuzz/fuzz_targets/<name>.rs` here, a copy of any other target calling
   your function, and a `[[bin]]` entry for it in `fuzz/Cargo.toml`.
3. Optionally add seeds to `fuzz/corpus/<name>/`, then `./fuzz.sh <name> 600`.
4. Fixed crashes: a minimized input goes to `fuzz/regressions/<name>/` and
   to yaxml's `tests/fixtures/fuzz-regressions/<name>/` (map the directory to
   the function in `fuzz_regressions_replay` if it is not a plain XML target).
