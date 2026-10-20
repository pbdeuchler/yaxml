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
- `encoding/2cb101abf3950cb2d16e120a8388cdf3fa696b74`,
  `encoding/7af10d0ae984ef19d8db1268df1eec2d420f1374` (moved out of
  `corpus/encoding` 2026-10-20; they failed `./coverage.sh` on core main
  `cdc363b54`): DOCTYPE documents with entity references (an internal
  entity whose value references another; an external subset with a PE in
  CDATA) under the hash-derived `DecodeLimits` of `exercise_decode_limits`
  (bytes 6 and 7: `max_decoded_bytes` 89 and `max_decoded_token_bytes` 80
  for the first) are accepted when fed whole and rejected when fed in
  4 KiB-or-smaller chunks, or the reverse: `encoding.rs` "decoded ceilings
  depend on chunk boundaries".
- `textreader/*` (seven inputs, moved out of `corpus/textreader` the same
  day, same reason): entity declarations whose replacement text holds
  character references (`&#x26;#xA;`), read with `TextReader`; the
  driver's `content` check reparses `read_outer_xml` without the DTD and
  gets `UnknownEntity` (textreader.rs "outer XML reparses").
- `dtd/xinclude-changes-acceptance-tight-limits.bin` (2026-10-20 campaign,
  `dtd` target; reproduces with core main's `src/`): control `0xcb` (the
  resolver wrapper, template 2, limits profile 3: small `max_dtd_bytes`,
  `max_owned_bytes` and similar ceilings), document `<!DOCTYPE r><r/>`. The
  owned parse succeeds and the same parse with `XINCLUDE` and no include
  element fails (resolver.rs "XINCLUDE changed acceptance"): XInclude
  processing seems to spend a ceiling the plain parse does not.
