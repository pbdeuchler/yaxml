# Open fuzz findings

Unfixed reproducers live here, outside `fuzz/regressions/`, because yaxml's
`tests/fuzz_smoke.rs::fuzz_regressions_replay` replays those (its copy in
`tests/fixtures/fuzz-regressions/`) and requires every case to pass. Move a
file to both once fixed. Replay:
`fuzz/target/<triple>/release/<target> open/<target>/<file>` (after
`cargo fuzz build`) or `cargo run --release --example fuzz_replay -- <file>`.

Open findings (2026-10-19 coverage campaign); each has a note beside it:

- `dtd/laughs-slow-unit.xml`: about 2 s per run through the `dtd` driver
  under ASan (bounded expansion; a slow unit, not a hang).
- `dtd/laughs-slow-unit-2.xml`, `encoding/laughs-slow-unit.xml` (the same
  1.6 KiB input, 2026-10-19 coverage pass 2): the bounded billion-laughs
  family again, 2.4 s through `dtd` and 6 s through `encoding` (every
  decoding surface) under ASan on a loaded host; `-timeout=2` stops on it.
- `dom/recover-defaulted-empty-prefix-binding.xml` (`RECOVER | DTDATTR`):
  an ATTLIST default `xmlns:p ""` is a fatal `Namespace` error in yaxml
  (`ParserState::recover_specified` keeps errors in defaulted attributes
  fatal), so recovery ends without the root and the saved document does
  not reparse to itself. libxml2 2.15.4 reports nothing for the defaulted
  binding and keeps `xmlns:p=""` (strict and `--recover`).

The entity-amplification, event-bound, output-bound, buffered invalid-markup
and DOM text-join reproducers from the seeded corpus run were fixed and moved
to `fuzz/regressions/`, as were `dom/recover-ignore-enc-utf7-save.bin` (`IGNORE_ENC` records no
document encoding unless a BOM auto-detected one, as libxml2's
`xmlGetActualEncoding`) and `save/entity-value-escaped-reference.bin`: a
single-byte encoding lacking a character of a reference name inside an entity
value is now `InvalidCharacter`, where libxml2 writes `&&#N;...;`, which does
not reparse.
- `owned/recover-not-fixed-point-pi-attlist.xml` (2026-10-19, `owned`
  target, found while fuzzing diagnostics): a leading non-UTF-8 byte, an
  unterminated `<?xm <"...` PI whose quoted text runs into a later XML
  declaration and DOCTYPE; `RECOVER` output reparses to a different document
  (options.rs "recovery is not a fixed point"). Reproduces on main
  `d30e423fd`; diagnostics are not involved.
