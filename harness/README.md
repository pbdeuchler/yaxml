# harness

Benchmarks, libxml2 2.15.4 differential tests and the W3C XML Test Suite run
for yaxml. It is its own Cargo workspace (path dependency `..`), so `cargo
test` at the repository root never builds it. Run it through the root
scripts (`./setup.sh`, `./oracle.sh`, `./bench.sh`); the
[root README](../README.md#benchmarks-differentials-and-fuzzing) has the
prerequisites and commands. Fuzzing is in [`../fuzz`](../fuzz/README.md).

- `tests/`: differential tests (`*_oracle.rs`, `xsd_suite.rs`) and tests of the
  tools below. Without the oracle environment `cargo test` here skips or
  ignores the differentials. The C helpers they drive are
  `tooling/libxml2_*.c`; `../oracle.sh` compiles them and passes their paths
  in `YAXML_*_HELPER` variables.
- `examples/`: the conformance runner (`xml_conformance`), benchmark drivers
  (`parse_benchmark`, `structure_benchmark`, `adversarial_scaling`,
  `reset_bench`), oracle programs and `fuzz_replay`.
- `benches/`: yaxml-only throughput benchmarks (`cargo bench`).
- `lib.sh`: locations and helpers shared by the root scripts.

Build output goes to `harness/target/` and results to `harness/results/`.
