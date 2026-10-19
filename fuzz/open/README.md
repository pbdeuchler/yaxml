# Open fuzz findings

Unfixed reproducers live here, outside `fuzz/regressions/`, because
`tests/fuzz_smoke.rs::fuzz_regressions_replay` replays that tree and requires
every case to pass. Move a file there once fixed. Replay:
`fuzz/target/<triple>/release/<target> open/<target>/<file>` (after
`cargo fuzz build`) or `cargo run --release --example fuzz_replay -- <file>`.

There are currently no open findings. The entity-amplification, event-bound,
output-bound, buffered invalid-markup and DOM text-join reproducers from the
seeded corpus run were fixed and moved to `fuzz/regressions/`.
