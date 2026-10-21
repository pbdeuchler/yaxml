# Open fuzz findings

Unfixed reproducers live here, outside `tests/fixtures/fuzz-regressions/`
(which `fuzz/regressions` links to), because
`tests/fuzz_smoke.rs::fuzz_regressions_replay` replays those and requires every
case to pass. Move a file there once fixed. Replay:
`fuzz/target/<triple>/release/<target> open/<target>/<file>` (after
`cargo fuzz build`) or, in `harness/`,
`cargo run --release --example fuzz_replay -- <file>`.

Open findings:

- `push/recover-byte-charset-reference-offset.xml` (2026-10-20, `push`
  target, `-fork=1` 300 s run on core `5cc5929e4`; reproduces on main
  `37f104e10`): under `RECOVER`, in an ISO-8859-1 document, the
  `InvalidReference` for `&` followed by byte `FF` in an attribute value is
  reported one byte earlier by the stream than by the slice reader
  (`tests/support/fuzz/recover.rs` "stream/slice recovery differ"); `FF`
  decodes to two UTF-8 bytes, so one side maps a decoded offset.

The entity-amplification, event-bound, output-bound, buffered invalid-markup
and DOM text-join reproducers from the seeded corpus run were fixed and moved
to `fuzz/regressions/`, as were `textreader/outer-xml-entity-in-attribute.xml`
(fixed on main by the outer-XML work), `dom/recover-defaulted-empty-prefix-binding.xml`
(a defaulted declaration that cannot bind is now dropped, and namespace errors
never stop a parse), `dom/recover-ignore-enc-utf7-save.bin` (`IGNORE_ENC` records no
document encoding unless a BOM auto-detected one, as libxml2's
`xmlGetActualEncoding`) and `save/entity-value-escaped-reference.bin`: a
single-byte encoding lacking a character of a reference name inside an entity
value is now `InvalidCharacter`, where libxml2 writes `&&#N;...;`, which does
not reparse. Fixed 2026-10-20 and moved to `fuzz/regressions/`:
`encoding/internal-entity-reference-chunks` and
`encoding/external-subset-reference-chunks` (were `2cb101ab...`, `7af10d0a...`:
a general reference read in place in a DOCTYPE document fit a token ceiling
one byte shorter than the same reference collected across chunks) and
`dtd/xinclude-changes-acceptance-tight-limits.bin` (XInclude's walk counted
the document node against `max_events`, which the parse does not).

Fixed 2026-10-20 (`work/openfinds2`) and moved to `fuzz/regressions/`:
`dom/entity-cut-off-child-range.xml` and
`dom/entity-misnamed-declaration-child-range.xml` (a parse stopped by an
error inside entity replacement text closed the open elements at the error's
offset, the reference's start, so the element holding the `EntityRef` node
ended before it; they now close past the reference) and
`owned/recover-not-fixed-point-pi-attlist.xml` (on the DTD stack a declared
attribute's value holding `<` was emptied under `RECOVER`, its scanner's
error recorded twice; libxml2 keeps the value, and its output reparses to
itself; yaxml now matches it byte for byte). Closed as linear: the
billion-laughs slow units (`dtd/laughs-slow-unit*.xml`,
`encoding/laughs-slow-unit.xml`, the latter identical to
`dtd/laughs-slow-unit-2.xml`) build 73,729 nodes (92 KiB of text) from 1.6
KiB within the default amplification allowance; release time through the
`dtd` driver is 0.47 s (`encoding` 0.73 s), and with the root referencing
`ha44`..`ha48` (each doubling the expansion) the marginal instructions per
step are 0.40, 0.79, 1.64, 3.21 G (`dtd`) and 0.57, 1.20, 2.52, 5.05 G
(`encoding`): linear, the time is the work every surface does per node.
`dtd/laughs-slow-unit.xml` itself fails fast (1.7 ms) and stays as
`regressions/dtd/laughs-undeclared-step.xml`; the 1/16-size `ha44` variant
is `regressions/dtd/laughs-linear-ha44.xml` (the full one costs 5 s per
replay in a debug build).
The same 300 s `encoding` run wrote a `-timeout=2` artifact of this family
(`laugh0`..`laugh13`, 1.3 KiB): 3.9 s under ASan, 0.35 s in release; not
kept. Bounded billion-laughs inputs need `-timeout=5` under ASan.
