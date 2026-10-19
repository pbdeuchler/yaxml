# Open fuzz findings

Unfixed reproducers live here, outside `fuzz/regressions/`, because
`tests/fuzz_smoke.rs::fuzz_regressions_replay` replays that tree and requires
every case to pass. Move a file there once fixed. Replay:
`fuzz/target/<triple>/release/<target> open/<target>/<file>` (after
`cargo fuzz build`) or `cargo run --release --example fuzz_replay -- <file>`.

Open findings (2026-10-19 coverage campaign); each has a note beside it:

- `save/entity-value-escaped-reference.bin`: a single-byte save encoding
  escapes a non-ASCII entity-reference name inside an entity value, so the
  output no longer reparses.
- `dtd/laughs-slow-unit.xml`: about 2 s per run through the `dtd` driver
  under ASan (bounded expansion; a slow unit, not a hang).

The entity-amplification, event-bound, output-bound, buffered invalid-markup
and DOM text-join reproducers from the seeded corpus run were fixed and moved
to `fuzz/regressions/`.
