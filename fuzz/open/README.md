# Open fuzz findings

Unfixed reproducers live here, outside `fuzz/regressions/`, because
`tests/fuzz_smoke.rs::fuzz_regressions_replay` replays that tree and requires
every case to pass. Move a file there once fixed. Replay:
`fuzz/target/<triple>/release/<target> open/<target>/<file>` (after
`cargo fuzz build`) or `cargo run --release --example fuzz_replay -- <file>`.

Open findings (2026-10-19 coverage campaign); each has a note beside it:

- `dom/recover-ignore-enc-utf7-save.bin`: under `RECOVER | IGNORE_ENC` a
  document declaring UTF-7 parses, but saving with the declared label is
  `UnsupportedEncoding`, which `check_recovered` treats as a failure.
- `dtd/laughs-slow-unit.xml`: about 2 s per run through the `dtd` driver
  under ASan (bounded expansion; a slow unit, not a hang).

The entity-amplification, event-bound, output-bound, buffered invalid-markup
and DOM text-join reproducers from the seeded corpus run were fixed and moved
to `fuzz/regressions/`, as was `save/entity-value-escaped-reference.bin`: a
single-byte encoding lacking a character of a reference name inside an entity
value is now `InvalidCharacter`, where libxml2 writes `&&#N;...;`, which does
not reparse.
