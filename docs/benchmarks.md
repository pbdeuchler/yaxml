# Parser, serialization and mutation benchmark evidence

Matched parsing and structural runners compare yaxml with pinned libxml2;
other runners measure yaxml alone. Results apply to their recorded workloads
and lifecycles. Build release binaries before recording runs; debug execution
under `cargo test --all-targets` is only a functional smoke check. The
`cargo` commands and relative paths below run in `harness/` (its own Cargo
workspace); `./bench.sh` at the repository root wraps the matched parse
comparison.

## Matched DOM and streaming parsing

The `parse_bench` example and external `harness/tooling/libxml2_parse_bench.c` helper
implement the [matched parsing contract](archive/plans/matched-parse-baseline.md).
They measure fresh borrowing/owned DOMs, memory readers, SAX push input and
buffered pull input with complete logical-value observations. DOM traversal
has its own clock; streaming observes values during parsing. Both include
destruction separately. Neither includes process startup or fixture loading.

Compile the C helper against the [pinned oracle](oracle.md), then build both
Rust examples with `cargo build --release --example parse_bench --example
parse_benchmark`. Invoke the driver with prebuilt executable paths:

```sh
target/release/examples/parse_benchmark \
  --libxml2-helper /path/to/libxml2-parse-bench \
  --yaxml-helper "$PWD/target/release/examples/parse_bench" \
  --fixture namespaces --mode push --iterations 20 --samples 7 --chunk-size 8192
```

Fixtures are `tiny`, `flat`, `text`, `attributes`, `namespaces`, `normalized`,
`unicode`, `deep` and `mixed`, plus the DOCTYPE family `dtd-decl`, `dtd-refs`,
`dtd-nested`, `dtd-attrs` and `dtd` run under the helpers' `dtd` profile (see
[DTD-heavy parsing](#dtd-heavy-parsing-2026-10-20-dtd-fixture-family)). Select
`--input PATH` instead of `--fixture` for external UTF-8, XML 1.0, DTD-free
bytes, or DOCTYPE bytes with `--profile dtd`. Modes are `dom`, `owned`, `reader`,
`push` and `buffered`; `owned` compares to the reference DOM. The driver checks
engine/version identity, requested work, six semantic counters and the digest
against both the pair and preflight before publishing a retained pair. It
alternates order and fails on diagnostics, invalid rows, incomplete work,
observation drift or input changes. The literal transcript test separately
anchors counters and digest; differential agreement alone is insufficient.

The reference DOM owns strings where the borrowing DOM can retain input slices.
`xmlReaderForMemory` copies the supplied input, while Rust's slice reader
borrows it. Buffered input caps each C read callback at the chosen chunk size;
the Rust caller uses that capacity for its `BufReader`. Their internal buffering
and event fragmentation differ. Finite default parser policies also differ.
These are explicit lifecycle comparisons for inputs accepted by both sides.

The handshake deliberately permits debug helpers for functional testing. Before
publishing performance, use an immutable release checkpoint and record exact
source/binary/input hashes, build flags, reference build and host. Confirm
identities again after all samples. A successful protocol exchange alone is
neither optimized-build evidence nor a performance result. Broader size/chunk
series, failure costs, latency, allocations and memory remain required.
The [first matched parsing validation](archive/validation-parse-baseline.md) records
65 workloads with raw phase distributions and substantial remaining gaps.

`cargo bench --bench parse` retains the pre-pull DOM parse/destruction, owned
parse/destruction, slice events and 8 KiB push workloads; its `dtd` fixture
expands an internal-subset entity and applies attribute defaults and
normalization in every element, so the DTD path is measured too. Set
`YAXML_BENCH_ITERATIONS` to fix their batch size. Record this runner before and
after the shared-stream refactor with identical fixture bytes and settings.

### Safe SIMD kernels (2026-10-26, from `bdb4788e0`)

`#![forbid(unsafe_code)]` rules out `core::arch` intrinsics: calling a
`#[target_feature]` intrinsic needs `unsafe` even with NEON enabled, and
`std::simd` is nightly-only. What is left is plain Rust that LLVM
vectorizes. A fixed block from `as_chunks::<N>()` folded with a bool
`any | test(b)` compiles to NEON compares, `orr` and one `umaxv`. Byte
masks built with `wrapping_neg` and plain-Rust movemask emulation (about
60 instructions per 16-byte mask) don't vectorize well.

Equivalence: a digest tool linked against `bdb4788e0` and this tree hashes
1,884,742 outputs:

- inputs: 17,040 synthetic files with special bytes at block edges in text,
  attributes, comments, CDATA and the prolog; the benchmark fixtures; the
  fuzz corpus; `tests/fixtures`; XMLTS;
- flags: 0, RECOVER, NOENT, NOENT|DTDATTR, NOBLANKS, RECOVER|NOENT;
- surfaces:
  - DOM and owned `save::to_vec` plus `errors()`, and owned saves in UTF-8,
    ISO-8859-1, UTF-16 and US-ASCII, each with and without FORMAT;
  - slice-reader events with `position()`;
  - push at chunk sizes 1, 7 and 64 KiB, with origins;
  - a buffered reader at capacity 13;
  - `encode_entities` (Xml, Ascii, Html) and `encode_special_chars`.

Every output is byte-identical. Planted screen bugs give 52,235 diffs
(`invalid_char`) and 92 diffs (a `\r` dropped from the save screen), so the
inputs reach the block paths.

- `escape::invalid_char` (every token; whole slice-reader input). Text
  shorter than 16 bytes still uses the table. Longer text is screened in
  128-byte blocks, then 64-byte blocks, for C0 controls and `EF`. A hit is
  re-checked with a branch-free test for `EF BF BE/BF` over three shifted
  views of the block, because `EF` alone is common (U+F000..U+FFFF, CJK
  punctuation). Only a confirmed block is searched.
- `escape::find_byte` (shared by `save::Out::text` and `save::escape_into`):
  - Runs under 16 bytes, and a marked first byte, use the exact test.
  - Longer runs are screened in 16-byte blocks by a superset test, and a
    block that passes is searched exactly.
  - `Out::text` gets exact `SPECIAL` tables from the same `const fn` as its
    screen.
  - A run of non-ASCII characters is written by the out-of-line
    `Out::encoded` in one loop, instead of one `chars().next()` per
    character.
  - `char_reference` is inlined, so hex references divide by a constant 16.

Parse, instructions (min of 3):

| Workload | Instructions |
|---|---:|
| dom cjk (576 KB, CJK text) | -41.70% |
| push cjk | -40.49% |
| reader text | -6.43% |
| push text | -6.36% |
| dom text | -5.55% |
| svg, unicode, mixed, push svg/7, buffered svg, owned attributes, tiny | within ±0.24% |

Save and escape (owned document parsed once, `to_vec` repeated; `save` =
default options, which escape non-ASCII as `&#x..;` when no encoding is
declared; `save_utf8` = UTF-8 output):

| Workload | Instructions | Cycles |
|---|---:|---:|
| save text | -79.43% | -66.26% |
| save cjk | -34.85% | -33.54% |
| save unicode | -11.70% | -14.36% |
| save_utf8 cjk | -58.33% | -50.54% |
| save svg | +0.21% | -1.45% |
| save attributes | +0.26% | +0.77% |
| save mixed (1–3 byte texts) | +1.62% | +0.28% |
| save tiny | -0.33% | -2.85% |
| `encode_entities` text | -77.66% | -68.35% |
| `encode_entities` attributes | -73.47% | -62.91% |
| `encode_entities` svg | -19.43% | -20.92% |
| `encode_entities` Ascii cjk | -37.51% | -33.93% |
| `encode_entities` Html mixed | -5.57% | -5.00% |
| `encode_entities` tiny | -3.88% | -5.13% |

Measured and dropped:

- SWAR movemask for `tokenizer::tag_close`: 2.5× slower than `memchr`.
- A cheap superset screen for `invalid_char` without the `EF BF` re-check:
  pretty-printed documents have a newline or `EF` in most 64-byte blocks
  (svg 51%, dtd 98-100%).
- A `find_byte` block loop written as `filter` + `find_map`, or as a `for`
  loop with a running offset: `position` over the blocks is 5 points better
  on save text.
- A screen-only first check, which looked up the first byte twice
  (cjk +8.6%).
- The non-ASCII run inlined into `Out::text`: cjk -37%, but text, svg and
  attributes +2%.
- `find_byte` for the attribute arm of `needs_normalization` (one pass
  instead of `memchr3` + `memchr`): attributes, svg, namespaces and tiny
  -0.03..-0.33% instructions. Too small to keep.

`from_utf8` can't be skipped or fused with the screen: it is the only safe
way to get a `&str`.

### Start tags and node links (2026-10-22, from `10757c81c`)

Instruction counts (`instructions retired` from `/usr/bin/time -l`, min of
3) of a parse-only driver linked against `10757c81c` and this change, same
fixtures, identical output; rustc 1.97.0, thin LTO, 1 CGU. The host ran at a
load average near 29, so cycles are not reported. Changes:

- `TagScanner::scan_name` is one table pass: `NAME_BYTES` gives `:` the
  START and CONTINUE classes, so validity is one AND of classes and the
  QName split uses the last colon seen in the pass.
- The duplicate-attribute check takes a single attribute without comparing,
  and up to `SMALL_ATTRIBUTES` attributes by `(namespace ID, local name)`
  pairs. `resolve_attribute` skips resolution for unprefixed names other
  than `xmlns`. Two or more attributes go to the out-of-line
  `resolve_attributes`: with the pair array inline in `start_element`,
  attribute-free documents paid for it (reader mixed +1.24%, bisected;
  out of line -0.64%, svg/attributes unchanged).
- `dom::NodeId` stores index + 1 in a `NonZeroUsize`, so each of a record's
  four `Option<NodeId>` links is one word (32 B less per record).
  `OwnedNodeId` keeps its raw slot, so `from_parts` still accepts any slot.

| Workload | Instructions |
|---|---:|
| dom svg | -5.86% |
| dom attributes | -4.64% |
| dom flat | -2.06% |
| dom tiny | -1.08% |
| dom mixed | -0.82% |
| dom namespaces | -0.56% |
| dom unicode | -0.32% |
| owned svg | -4.39% |
| owned flat | -1.07% |
| owned namespaces | -0.37% |
| owned tiny | -0.38% |
| reader svg | -6.32% |
| reader flat | -1.95% |
| reader namespaces | -0.69% |
| push svg/65536 | -5.48% |
| push namespaces/7 | -1.17% |
| push flat/7 | -1.05% |

Peak RSS of a DOM parse of the 2.5 MB SVG fixture drops from 40.98 to
38.42 MB (-6.2%).

Measured and rejected (don't retry):

| Change | Result |
|---|---|
| Reserve attributes from a count of `=` in the tag | svg +1% (the first growth already reserves 4) |
| 256-entry class table in `short_value` | attributes +1.1% |
| Computing `local` eagerly in the duplicate check, or the pair array without the single-attribute case | flat +0.9% |
| Duplicate check for 2+ attributes inline in `start_element` | reader mixed +1.0% (no attributes) |
| Out-of-line colon split (memchr / scalar) | namespaces +0.9% / +1.4% |
| `&raw[value_start..][..length]` value slice | worse than `&raw[value_start..cursor]` |
| Owned nodes reserved from a full `<` count (as `Document` does) | owned svg -7.1%, but text +8.6%, mixed +1.5%, svg peak RSS +34% |

The owned builder keeps its sampled estimate (two nodes per `<` in the
first 16 KiB, scaled to the input) but no longer caps it at 4096 nodes,
only at `max_events`: owned svg -3.20%, mixed -3.01%, flat -1.33%,
namespaces -1.07%, text and tiny within 0.13%; svg peak RSS 116 -> 72 MB
(-38%; regrowth held old and new buffers at once), mixed -23%, flat -10%.
Unlike the full count, the sample keeps the scan off text-heavy input.

`publish_replacement` and `publish_physical` are `inline(always)`, as PGO
inlines them: NOENT DOM dtd-nested -2.15%, dtd-refs -2.60%, dtd -0.22%; no
other row moved by more than 0.4%. Making `reserve_frame` `inline(always)`
on top of that changed nothing. `ContentControllerCore::next_stable` checks
its pending entity marker before taking it (an empty `Option::take` still
moved the slot for every token): NOENT dtd-nested dom -1.65%, owned -1.58%,
reader -2.17%, dtd-refs dom -1.41%. The same guard in `acknowledge`
(streamed path) moved nothing.

Profile-guided optimization (driver build only, `-Cprofile-generate` /
`-Cprofile-use` on rustc 1.97.0, merged with the Command Line Tools'
`llvm-profdata`):

| Training set | Rows trained on | Held-out rows |
|---|---|---|
| Every row measured here | -3% (dom mixed) to -28% (buffered dtd-nested); dom svg -12.5%, push svg -25.6%, dom dtd -15.8% | — |
| attributes, unicode, normalized, dtd-attrs only | — | dom/push/reader -5..-11%; owned svg +5.9%, push dtd-refs +4.7%, dom mixed +3.1% |

PGO pays only when training covers every surface a build serves. Surfaces
it never saw get worse. That suits the binaries this repo builds
(`yaxmllint`, `yaxml-capi`, `yaxml-py`), not the library crate, which
downstream users compile themselves. Most of its gain is block layout and
hot/cold splitting, which source attributes can't express.

`./pgo.sh` builds the PGO `yaxmllint` (harness/target/pgo/use/release):
it trains on every fixture under 12 option sets (default, `--stream`,
`--noent`, `--noent --stream`, `--dtdattr`, `--loaddtd --valid`,
`--noblanks`, `--recover`, `--html`, `--c14n`, `--xpath`, `--format`), 20
documents a run, and prints each run's instructions against the plain
release build. Results, 2026-10-22, the trained runs themselves:

| Fixtures | Instructions |
|---|---|
| dtd family (5 fixtures x 12 sets) | -13% to -27% |
| flat, mixed, namespaces, normalized, unicode | -11% to -27% (`--format`/`--noblanks` most) |
| attributes | -6% to -19% (`--html`) |
| deep (900 B) | -3% to -30% (`--format`) |
| tiny (23 B, process-start dominated) | -1.3% to +0.3% |
| text (500 KB of character data, memchr-bound) | +1% to +8% (`--c14n` worst), -5% under `--html` |

DTD rows: the NOENT dtd-nested DOM gap (about 2x libxml2's instructions)
is per-reference replay. Every reference rescans its cached `Piece`s
through the tag scanner, with the work, frame and catalog ledgers per
token. libxml2 parses an entity once and copies its node list. Copying in
the DOM would mean rewriting each copy's origins (invocation and
reference), re-merging text at the boundaries, and charging every ledger
exactly as replay does. Buffered dtd-nested spreads its cost over the
session layers (prepare 14%, lend 9%, `lend_current` 7%, drive 7%+7%).
The scratch copy of each replacement token (2.5%) is forced by the
catalog borrow that publication takes. Merged-text and node growth in
NOENT DOMs (malloc about 8%) charge `max_owned_bytes` (text) or follow
expansion the `<` hint can't see.

Not attempted, with reasons:

- Pre-sizing the owned pool. Pool growth charges `max_owned_bytes`
  cumulatively, so another reservation schedule changes which documents
  pass near the limit.
- Removing the copies in `ParserState::bind`. Its charges feed the same
  ledger.
- Removing the per-element attribute `Vec` (malloc is about 11% of DOM svg).
  The public `Attributes::Owned(Vec)` layout fixes it.
- An end-tag compare-first path. `end_element` already compares against
  the open name before rescanning.

Buffered pull, three structural costs per event (64 KiB `BufReader`,
instructions; cycles moved the same way or more):

1. Stack probes. `StreamingSession::start_dtd` and the subset-to-content
   handover (`DtdSession::start_content`) were inlined into
   `prepare_inner`, which every buffered event calls. Their controller
   moves gave it an 18 KiB frame, probed page by page on each call.
   Out of line, the frame is 480 B. `Reader::read_session` had the same
   problem through `Reader::start_content`. Buffered DTD rows -3% to -5%;
   reader and DOM DTD rows -0.4% to -0.6%.
2. Event rewrapping. `lend_plain` moved its event through
   `and_then`/`ok_or_else`/`map_err`/`?`, and each layout change was an
   88 B copy, partly through misaligned 4-byte stores that stall store
   forwarding. A `match` and inlined `parse_event` (plain), `lend_current`
   and `lend` (DTD) cut those copies. Plain rows -1% to -5%; DTD rows
   -3% to -4%.
3. Unscreened copies. A buffered token must be copied out of
   `fill_buf`, because `position()` equals the bytes consumed from the
   source after every event. `adopt` copied it with `append_slice`, which
   leaves it unscreened, so `validate` ran `invalid_char` on every token.
   Tokens of 16 bytes or fewer are now copied and screened in one pass
   (`screened_push`, the small-run path's loop). mixed -7.8%, flat -4.4%;
   a 32 B threshold moved normalized -1.3% but namespaces +0.7% and
   text +0.6%.

Together, `buffered` against the start of the series:

| Fixture | 64 KiB | 7 B |
|---|---:|---:|
| mixed | -12.78% | -4.13% |
| flat | -7.99% | -3.07% |
| normalized | -5.81% | -2.20% |
| unicode | -4.53% | -1.90% |
| deep | -2.80% | -1.23% |
| namespaces | -2.72% | -1.59% |
| tiny | -1.71% | -1.67% |
| attributes, text | -0.1% to +0.4% | -0.2% |
| dtd-nested (NOENT) | -7.65% | -7.65% |
| dtd-refs (NOENT) | -7.35% | — |
| dtd (NOENT) | -6.07% | — |
| dtd-attrs (NOENT) | -4.19% | — |

DOM, owned, reader and push rows stayed within ±0.4%. Push tiny is +0.1%
at 100k documents, and its 20k-document runs drift ±0.7% with binary
layout.

#### Entity references after a DOCTYPE (same day)

Per-unit instructions (difference of 20- and 40-iteration runs, 20,000
units each, NOENT; the helper's dom/reader modes) on micro-fixtures:

| Unit | libxml2 dom | yaxml dom | libxml2 reader | yaxml reader |
|---|---:|---:|---:|---:|
| `<p>zz</p>`, no DOCTYPE | 2704 | 1762 | 2726 | 1441 |
| `<p>zz</p>` after a DOCTYPE | 2712 | 2441 | 2729 | 2092 |
| `<p>&z;</p>`, `z` = `zz` | 3379 | 3465 | 3287 | 3116 |
| `<p>&z;</p>`, `z` = `<q/>` | 3419 | 3772 | 3987 | 3382 |

So a single reference costs about 900-1000 units over the inline text,
spread over the entity frames, ledgers and publication layers with no
function above 8% self. `dtd-nested`'s remaining DOM gap is libxml2
copying an entity's parsed nodes on every later reference
(`xmlDocCopyNode`) where yaxml replays the cached pieces. Of a replayed
token, the parser's publication (`start_element`, `end_element`, name
scans, normalization checks) is about 7% of the DOM parse; the per-token
ledger calls (work, expansion, amplification, depth) are the rest. A
DOM-side subtree copy was first estimated to save at most 7-10%; recorded
expansions (below) replace the whole replay machinery instead, at the
event level.

Two small changes (`read_session` handles only the content phase, with the
DOCTYPE phase in a cold `read_declarations`; a resolved nested reference
no longer copies its spelling into `scratch` unless it keeps a reference
node), instructions:

| Fixture (NOENT) | DOM | Owned | Reader |
|---|---:|---:|---:|
| dtd-nested | -2.36% | -2.38% | -3.08% |
| dtd-refs | -0.63% | -0.58% | -0.75% |
| dtd | -0.46% | -0.42% | -0.50% |
| dtd-attrs | -0.29% | -0.26% | -0.36% |

DTD-free DOM and reader rows did not move (±0.16%).

#### Recorded expansions (same day)

Deleting every ledger call from the replay path saved only 2-7%
(instructions), so a single headroom counter could not pay on its own:
the cost is the replay machinery (pieces, frames, origins, publication).
Parsing `dtd-nested` with every `&x;` written out inline costs 46.29M
instructions per document against 101.12M with references (libxml2
49.97M).

src/dtd_controller/content/template.rs records the events an eligible
top-level reference publishes from stable input under NOENT, with their
origins and owned charges and what the expansion charged to every ledger,
and replays them at later references of the same entity. A replay first
checks that every ledger has room for the whole expansion. Within one
reference every counter only grows, so when the totals fit no check
inside could have failed, and the ledgers are charged at once. When they
don't fit, the reference is expanded as before, so the failing ledger,
error and offset stay the same. Owned bytes are charged per replayed event,
as the expansion charges them. Origins are renumbered from the new
reference and invocation. Expansions that depend on context are always
expanded: prefixed names (other than `xml`), namespace declarations,
instructions, references resolved by name, kept forms, errors or warnings.
A template is used only when the in-scope default namespace is identical,
including whether it is owned. Each recording is capped at 16 KiB, and all
recordings together at 1 MiB.

| Fixture (NOENT) | DOM | Owned | Reader |
|---|---:|---:|---:|
| dtd-nested | -37.19% (cycles -27.02%) | -37.45% (-26.09%) | -49.26% (-33.40%) |
| dtd-refs | -16.69% (-13.81%) | — | -21.02% (-17.36%) |
| dtd | -16.40% | -21.34% (-22.61%) | — |
| dtd-attrs, dtd-decl | -0.04%, -0.22% | — | — |

DOM `dtd-nested` is now 63.5M instructions per document (libxml2
49.97M, inline 46.29M). Without NOENT, the DTD rows gain 0.15-0.63%
instructions with cycles flat, and DTD-free rows stay within ±0.08%. An
inline guard in front of an out-of-line `reuse` body was worse
(+1.1-1.9%). tests/internal/templates.rs compares templates on and off
(the test-only `TEMPLATES` switch) on the reader and both DOMs: events,
origins, errors and saved output, for every value of each ceiling up to
256 below its threshold. Deliberately breaking any remaining precheck,
the owned charges, origin rebasing or invocation numbering fails it.

### Small-chunk push (2026-10-20, `37f104e` → `5cc5929`)

Ratios are **libxml2 median / yaxml median** of parse plus destruction per
document, main (`37f104e`) → this series (`5cc5929`), with the min–max of
the per-pair ratios after. Method as in the sixth series: release
`parse_bench` of both revisions and the static libxml2 2.15.4 helper under
`parse_benchmark`, both revisions back to back per workload, 9 alternating
pairs per 8 KiB row and 21 per small-chunk row; rustc 1.97.0. Apple M1 Max
shared with other jobs (load average 4-30 during the matrix, up to 100 while
tuning). Repeated runs of one binary move a row by about ±2%, so changes
under 3% on rows this series did not touch are noise. yaxml leads in **65
of 65** workloads (the DTD family is [below](#dtd-heavy-parsing-2026-10-20-dtd-fixture-family));
`mixed` push/7 goes from 0.93 to **1.09** and `namespaces` push/7 from
0.95 to **1.05**. Every small-chunk row is 2-18% faster; no 8 KiB row moved
beyond noise.

| Fixture | DOM | Owned | Reader | Push | Buffered |
| --- | ---: | ---: | ---: | ---: | ---: |
| tiny | 3.61 → **3.55** (3.48–3.65) | 2.19 → **2.22** (2.12–2.29) | 4.15 → **4.12** (4.06–4.25) | 1.50 → **1.62** (1.52–1.66) | 2.19 → **2.26** (2.23–2.31) |
| flat | 1.32 → **1.33** (1.31–1.35) | 1.18 → **1.17** (1.15–1.18) | 1.43 → **1.42** (1.41–1.45) | 1.03 → **1.02** (1.01–1.05) | 1.06 → **1.08** (1.06–1.09) |
| text | 2.94 → **2.92** (2.87–2.98) | 2.48 → **2.47** (2.40–2.59) | 1.29 → **1.29** (1.13–1.31) | 1.14 → **1.13** (1.12–1.15) | 1.28 → **1.27** (1.24–1.29) |
| attributes | 2.09 → **2.09** (2.03–2.13) | 1.79 → **1.79** (1.77–1.82) | 1.40 → **1.41** (1.40–1.42) | 1.08 → **1.09** (1.02–1.09) | 1.40 → **1.39** (1.38–1.41) |
| namespaces | 1.56 → **1.56** (1.54–2.16) | 1.33 → **1.32** (1.31–1.33) | 1.88 → **1.88** (1.87–1.91) | 1.10 → **1.10** (1.09–1.11) | 1.42 → **1.43** (1.42–1.44) |
| normalized | 1.59 → **1.57** (1.54–1.59) | 1.47 → **1.46** (1.44–1.51) | 1.75 → **1.73** (1.70–1.75) | 1.20 → **1.21** (1.20–1.23) | 1.48 → **1.49** (1.46–1.53) |
| unicode | 1.34 → **1.34** (1.33–1.37) | 1.18 → **1.16** (1.14–1.19) | 1.62 → **1.61** (1.60–1.63) | 1.01 → **1.01** (1.00–1.03) | 1.26 → **1.27** (1.26–1.28) |
| deep | 1.18 → **1.20** (1.16–1.26) | 1.11 → **1.10** (1.08–1.15) | 1.95 → **1.93** (1.90–2.08) | 1.05 → **1.06** (1.04–1.37) | 1.27 → **1.33** (1.31–1.41) |
| mixed | 2.18 → **2.17** (2.15–2.22) | 1.81 → **1.81** (1.75–1.87) | 2.69 → **2.73** (2.67–2.84) | 1.28 → **1.28** (1.25–1.38) | 1.75 → **1.72** (1.70–1.74) |
| dtd-decl | 1.25 → **1.27** (1.27–1.29) | 1.24 → **1.27** (1.24–1.39) | 1.38 → **1.42** (1.39–1.46) | 1.23 → **1.25** (1.23–1.28) | 1.25 → **1.27** (1.24–1.30) |
| dtd-refs | 0.55 → **0.55** (0.54–0.55) | 0.60 → **0.60** (0.60–0.61) | 0.79 → **0.79** (0.78–0.83) | 0.92 → **0.92** (0.90–0.93) | 0.40 → **0.42** (0.42–0.44) |
| dtd-nested | 0.36 → **0.36** (0.35–0.36) | 0.37 → **0.37** (0.37–0.38) | 0.66 → **0.66** (0.66–0.67) | 1.29 → **1.29** (1.28–1.33) | 0.39 → **0.41** (0.41–0.41) |
| dtd-attrs | 1.20 → **1.21** (1.19–1.22) | 1.07 → **1.08** (1.06–1.70) | 1.26 → **1.26** (1.21–1.27) | 0.88 → **0.87** (0.86–0.88) | 0.89 → **0.92** (0.91–1.20) |
| dtd | 0.59 → **0.59** (0.58–0.59) | 0.58 → **0.57** (0.55–0.57) | 0.76 → **0.75** (0.74–0.76) | 0.77 → **0.76** (0.76–0.77) | 0.50 → **0.53** (0.52–0.53) |

| Small chunks | Push 1 | Push 7 | Buffered 1 | Buffered 7 |
| --- | ---: | ---: | ---: | ---: |
| tiny | 1.80 → **1.92** (1.57–2.03) | 1.45 → **1.55** (1.46–1.61) | 2.68 → **2.72** (2.44–2.85) | 2.17 → **2.29** (2.20–2.41) |
| text | 1.02 → **1.14** (1.10–1.17) | 1.03 → **1.22** (1.21–1.29) | 2.24 → **2.34** (2.31–2.39) | 1.68 → **1.87** (1.85–1.88) |
| namespaces | 1.03 → **1.13** (1.12–1.14) | 0.95 → **1.05** (1.03–1.07) | 1.99 → **2.03** (2.00–2.07) | 1.53 → **1.67** (1.66–1.70) |
| normalized | 1.12 → **1.25** (1.23–1.27) | 0.99 → **1.15** (1.13–1.17) | 2.29 → **2.34** (2.28–2.38) | 1.68 → **1.83** (1.79–1.86) |
| mixed | 1.07 → **1.23** (1.21–1.26) | 0.93 → **1.09** (1.06–1.13) | 2.53 → **2.62** (2.57–2.67) | 1.80 → **1.99** (1.97–2.05) |

Retired instructions and cycles per document (millions; instructions are
the minimum, cycles the median, of the difference of N- and 2N-document
runs, `/usr/bin/time -l`, three runs; `tiny` in thousands):

| Workload | libxml2 | main | this series | main cycles | cycles |
| --- | ---: | ---: | ---: | ---: | ---: |
| tiny push 1 | 23.46k | 14.23k | 13.35k | 2.86k | 2.61k |
| tiny push 7 | 15.38k | 10.36k | 9.96k | 2.29k | 2.21k |
| text push 1 | 124.48 | 131.09 | 118.09 | 20.06 | 17.77 |
| text push 7 | 27.12 | 27.39 | 25.11 | 5.97 | 5.12 |
| namespaces push 1 | 82.07 | 81.63 | 75.13 | 15.18 | 13.75 |
| namespaces push 7 | 40.94 | 42.69 | 39.23 | 9.11 | 8.21 |
| normalized push 1 | 63.72 | 63.86 | 57.77 | 10.74 | 9.64 |
| normalized push 7 | 27.26 | 29.17 | 26.38 | 5.79 | 4.94 |
| mixed push 1 | 38.52 | 36.69 | 32.93 | 6.38 | 5.59 |
| mixed push 7 | 14.80 | 16.48 | 14.50 | 3.14 | 2.71 |

Instructions did not predict time on this path: the M1's CPU counters
(`xctrace` CPU Counters, "CPU Bottlenecks") show about 25% of `mixed`
push/7 cycles lost to instruction delivery, and several changes that
removed instructions were slower (below). Stalls were found from the
leaf addresses of an `xctrace` Time Profiler run of a throwaway example
feeding 7-byte chunks, symbolized with `atos -i` (inline chains) and read
against `otool -tv`; exact per-feed instruction traces came from lldb
single-stepping, as before. Root causes:

- **One append per scan.** The tokenizer appended each lexical step's run
  separately, so a 7-byte chunk crossing a markup prefix or a quote paid two
  or three appends. It now scans all steps first and appends
  `input[..cursor]` once per call (`classify` already reads the token as
  buffer followed by input).
- **A copy call per append.** Each run went through `extend_from_slice`,
  that is libsystem `memmove`, about 45 instructions plus a far call for
  seven bytes. Runs of up to 8 bytes are appended through `Vec::extend` of
  a mapped iterator, which also ORs each byte's `escape::suspect` class
  into `Tokenizer::screened`; a token built only from such runs skips
  `invalid_char`, which was about 5% of `mixed` and `namespaces` push/7
  cycles. Longer runs (and adopted tokens) clear the flag, so they are
  screened whole as before; `str::from_utf8` stays per token.
- **A store-forwarding stall.** `ready: Option<(TokenKind, TokenSpan)>`
  was built through misaligned 16-byte stack copies that the following load
  only partly overlapped; that one instruction was 7% of `mixed` push/7
  samples. The kind and the span are now separate fields.
- **Vector state for other encodings.** `SourceEncoding::source_length`
  inlined into the token loop hoisted NEON constants into every call's
  prologue (eight extra saved registers); the non-UTF-8 width is now out
  of line (`mixed` push/7 -4% instructions).
- **Frame size.** `validate` was a call per token and is now inlined
  (-3% instructions); the DTD session loop of `StreamingSession::drive`
  (only reached after a DOCTYPE) is out of line, so the plain stream's
  delivery frame saves fewer registers (-4% cycles on push/7).

Tried and rejected, by cycles: `inline(always)` on `advance_inner` (-1.3%
instructions, +4% time), `memchr` wrappers out of line, scalar loops for
haystacks under 16 bytes (mixed within noise), `drive_chunk` out of line
(+2% cycles on push/7, -2% on 8 KiB push), the scan state kept in a local
across steps, a one-pass scan of short tag rests, and copying short runs
with `extend(iter().copied())` (it becomes `memcpy` again).

Remaining: `namespaces` push is bounded by work shared with 8 KiB chunks
(push/8192 is 1.10): each `xmlns` binding copies its prefix and URI
(`ParserState::bind` under `copying_names`), two or three `malloc`/`free`
pairs per element, about 8% of `namespaces` push instructions; reusing the
binding strings' capacity would raise every `namespaces` push row.

### Current matched results (2026-10-19, sixth series)

Ratios are **libxml2 median / yaxml median** of parse plus destruction per
document: the fifth-series merge (`4a99833`, rustc 1.95) → main (`7b2fb0f`,
rustc 1.97.0 as pinned by rust-toolchain.toml), with the min–max of the
per-pair ratios after. Since the fifth series main gained DOM editing and
copies, structured diagnostics, streaming save and `TextWriter`, the public
DTD model (`valid::Dtd` owns entity storage), TextReader `NOBLANKS`, reset
and the boxed lent storage. Method as before: 9 alternating pairs per
workload with both revisions back to back; every small-chunk row, `flat`,
`unicode`, `deep`, `normalized`, `mixed` and `tiny` DOM/reader were repeated
with 21 pairs (`mixed` buffered/1 and `namespaces`/`normalized` DOM a third
time), which the table shows. Apple M1 Max shared with other jobs (load
average 9-28). yaxml leads in **63 of 65** workloads; the losses are the
known `namespaces` push/7 (**0.98**) and `mixed` push/7 (**0.94**). No row
lost more than 2% against the fifth series; `normalized` DOM and owned are
1.8% slower in both runs while retiring 1.7% and 0.8% fewer instructions.

| Fixture | DOM | Owned | Reader | Push | Buffered |
| --- | ---: | ---: | ---: | ---: | ---: |
| tiny | 3.48 → **3.69** (3.50–3.91) | 2.21 → **2.38** (2.33–2.47) | 3.99 → **4.23** (4.09–4.38) | 1.52 → **1.54** (1.46–1.62) | 2.21 → **2.23** (2.19–2.27) |
| flat | 1.34 → **1.33** (1.30–1.37) | 1.18 → **1.18** (1.16–1.20) | 1.42 → **1.41** (1.32–1.65) | 1.04 → **1.03** (1.01–1.06) | 1.05 → **1.04** (1.01–1.08) |
| text | 2.95 → **2.96** (2.89–3.02) | 2.56 → **2.56** (2.40–2.88) | 1.28 → **1.29** (1.26–1.30) | 1.13 → **1.16** (1.08–1.77) | 1.28 → **1.26** (1.09–1.30) |
| attributes | 2.10 → **2.09** (1.69–3.84) | 1.65 → **1.69** (1.60–1.75) | 1.41 → **1.42** (1.28–1.47) | 1.05 → **1.05** (1.04–1.06) | 1.38 → **1.39** (1.08–1.95) |
| namespaces | 1.57 → **1.56** (1.52–1.61) | 1.31 → **1.32** (1.30–1.35) | 1.89 → **1.88** (1.25–1.90) | 1.09 → **1.11** (1.08–1.12) | 1.42 → **1.42** (1.40–1.43) |
| normalized | 1.64 → **1.61** (1.57–1.63) | 1.51 → **1.49** (0.93–1.53) | 1.77 → **1.75** (1.72–1.81) | 1.22 → **1.21** (1.19–1.38) | 1.48 → **1.48** (1.45–1.53) |
| unicode | 1.34 → **1.33** (1.26–1.35) | 1.18 → **1.17** (1.14–1.30) | 1.62 → **1.61** (1.56–1.78) | 1.02 → **1.02** (0.98–1.04) | 1.26 → **1.25** (1.24–1.27) |
| deep | 1.16 → **1.16** (1.06–1.29) | 1.06 → **1.08** (1.06–1.11) | 1.80 → **1.79** (1.74–2.01) | 1.06 → **1.08** (0.99–1.09) | 1.26 → **1.26** (1.23–1.41) |
| mixed | 2.21 → **2.22** (2.14–2.29) | 1.79 → **1.83** (1.72–1.89) | 2.67 → **2.65** (2.61–2.70) | 1.27 → **1.26** (1.25–1.43) | 1.69 → **1.68** (1.56–1.71) |

| Small chunks | Push 1 | Push 7 | Buffered 1 | Buffered 7 |
| --- | ---: | ---: | ---: | ---: |
| tiny | 1.91 → **1.93** (1.86–2.04) | 1.48 → **1.48** (1.04–1.56) | 2.69 → **2.74** (2.42–2.85) | 2.18 → **2.23** (2.07–2.29) |
| text | 1.22 → **1.25** (1.23–1.27) | 1.12 → **1.15** (1.08–1.17) | 2.23 → **2.24** (2.21–2.27) | 1.66 → **1.66** (1.64–1.74) |
| namespaces | 1.13 → **1.15** (1.08–1.16) | 0.97 → **0.98** (0.95–0.99) | 1.99 → **2.00** (1.97–2.03) | 1.51 → **1.52** (1.50–1.54) |
| normalized | 1.27 → **1.30** (1.28–1.31) | 1.05 → **1.06** (1.04–1.07) | 2.27 → **2.31** (2.26–2.35) | 1.67 → **1.68** (1.65–1.70) |
| mixed | 1.19 → **1.23** (1.15–1.28) | 0.93 → **0.94** (0.87–0.97) | 2.37 → **2.44** (2.40–2.55) | 1.78 → **1.79** (1.59–2.83) |

Compiler and code effects are separated by instruction counts (per
document, difference of N- and 2N-document runs, minimum of 3) of both
revisions built with both compilers (`cargo +1.95.0 ... --ignore-rust-version`
for main). Code changes moved no workload by more than +0.9% at a fixed
compiler; 1.97 retires 1-2.5% fewer instructions than 1.95 on slice-reader,
DOM and buffered/1 rows and up to +0.9% more on `attributes` push. The
`tiny` reader drop is the boxed lent storage (`141e20b`). Timing shows no
compiler regression at this load: the matrix is within noise of the fifth
series everywhere except the `tiny` DOM, owned and reader rows, 6-8% faster (boxing).

| Workload | 4a99 1.95 | 4a99 1.97 | main 1.95 | main 1.97 | code (1.97) | compiler (main) |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| tiny reader 8192 | 6.21k | 6.17k | 5.95k | 5.83k | -5.56% | -1.95% |
| tiny owned 8192 | 12.16k | 11.85k | 11.86k | 11.68k | -1.39% | -1.49% |
| flat dom 8192 | 14.29M | 13.97M | 14.25M | 13.97M | -0.00% | -1.97% |
| flat push 8192 | 11.16M | 11.12M | 11.18M | 11.14M | +0.13% | -0.36% |
| attributes push 8192 | 16.70M | 16.85M | 16.76M | 16.90M | +0.29% | +0.88% |
| namespaces reader 8192 | 23.98M | 23.59M | 23.63M | 23.42M | -0.72% | -0.88% |
| normalized dom 8192 | 22.21M | 21.83M | 22.07M | 21.83M | +0.01% | -1.09% |
| deep reader 8192 | 189.35k | 184.18k | 188.26k | 183.84k | -0.18% | -2.35% |
| mixed reader 8192 | 7.90M | 7.76M | 7.91M | 7.76M | -0.06% | -1.92% |
| mixed push 8192 | 7.49M | 7.54M | 7.56M | 7.58M | +0.53% | +0.23% |
| comments reader 8192 | 4.59M | 4.50M | 4.58M | 4.46M | -0.72% | -2.55% |
| cdata dom 8192 | 5.26M | 5.08M | 5.21M | 5.10M | +0.41% | -2.22% |
| text buffered 1 | 126.18M | 123.14M | 125.59M | 122.66M | -0.39% | -2.33% |
| mixed push 7 | 15.83M | 15.79M | 15.86M | 15.82M | +0.17% | -0.24% |
| namespaces push 7 | 41.48M | 41.15M | 41.39M | 41.16M | +0.00% | -0.57% |
| normalized push 7 | 28.13M | 28.08M | 28.13M | 28.09M | +0.03% | -0.14% |

DOM paths outside the matrix (`dombench`, a throwaway example; instructions
per iteration, same method), fifth series → main → this series, all rustc
1.97. `decl` is 2,000 internal general entities and `<r/>`; `many` adds a
reference to each; `dtd` is the `cargo bench --bench parse` fixture; save
is `save::to_vec` of an owned document (now through `to_writer`'s
32 KiB-chunked sink); XPath runs on an owned document.

| Workload | 4a99833 | main | after |
| --- | ---: | ---: | ---: |
| owned parse, decl | 27.65M | 30.58M (+10.6%) | 27.58M (-0.3%) |
| DOM parse, decl | 27.62M | 30.53M (+10.5%) | 27.58M (-0.1%) |
| owned parse, many | 74.28M | 77.12M (+3.8%) | 74.17M (-0.2%) |
| owned parse, dtd | 300.07M | 300.49M (+0.1%) | 300.68M (+0.2%) |
| DOM parse, dtd | 289.46M | 290.28M (+0.3%) | 290.37M (+0.3%) |
| save flat / mixed / namespaces | 6.85M / 5.73M / 9.84M | +0.3% / -0.7% / +0.9% | — |
| save dtd (kept entity refs) | 20.69M | -0.5% | — |
| XPath `count(//item[@a='value'])` flat | 18.67M | 0.0% | — |
| XPath `count(//p:item/q:leaf)` namespaces | 9.07M | 0.0% | — |

Root cause of the one regression: both DOM builders cloned the catalog's
`Dtd` into the document. Once `valid::Dtd` owned all declaration storage,
that clone copied every entity's name and value and rebuilt the `general`,
`parameter` and `declared` tables, about 1,500 instructions per entity.
The builders now move the finished catalog's `Dtd` out
(`Reader::take_dtd`, `StreamingSession::take_dtd`), and entity insertion
skips the lookups that `apply`'s duplicate check already excludes.

Remaining: DTD documents are far from libxml2. `xmllint --noout` (startup
subtracted) retires about 100M instructions on the `dtd` fixture against
yaxml's 300M owned, and about 10M against 27.6M for 2,000 entity
declarations. Samples spread over the entity-expansion machinery
(`TextCoordinator::drive`, `Tokenizer::advance_content` on replacement
text, `ContentControllerCore::take_event`/`enter_general`), allocation, and
SipHash (about 8%, the `valid::Dtd` and attribute-declaration tables); each
reference re-tokenizes its replacement text where libxml2 copies the
entity's cached node list. No single hot spot; closing it is a design
change, not a tuning pass (done in [DTD-heavy parsing](#dtd-heavy-parsing-2026-10-20-dtd-fixture-family)). `mixed` push/7 retires 6.7% more instructions
than libxml2 (15.82M against 14.82M) for the reasons in the fifth series;
no new lead.

### DTD-heavy parsing (2026-10-20, `dtd` fixture family)

Five DOCTYPE fixtures joined the matrix (`harness/examples/support/parse_benchmark.rs`):
`dtd-decl` (2,000 general entities and 1,000 element/ATTLIST pairs, `<r/>`),
`dtd-refs` (5,000 paragraphs with three internal references each, one with
markup, one with a character reference), `dtd-nested` (5,000 references to
an entity nesting two levels of markup and text references), `dtd-attrs`
(20 ATTLISTs with four CDATA defaults, NMTOKENS normalization and an ID,
5,000 elements) and `dtd`, the `cargo bench --bench parse` document. Both
helpers run them under the `dtd` profile: DOCTYPE accepted and entities
substituted (`NOENT`), so every mode observes the same expanded content. The
libxml2 push mode keeps the SAX2 DTD handlers; neither side's tree applies
defaulted attributes without `DTDATTR`, and the observers ignore them.
`--profile dtd` selects the profile for `--input`.

Instructions per document (difference of N- and 2N-document runs, minimum
of 3, chunk 8192) for libxml2, main (`3b00e38` with this harness) and
after (`6c9b4d3`, main merged); ratio is libxml2/yaxml.

| Fixture | Mode | libxml2 | main | after | ratio | change |
| --- | --- | ---: | ---: | ---: | ---: | ---: |
| dtd-decl | dom | 21.97M | 56.21M | 26.13M | 0.39 → **0.84** | -53.51% |
| dtd-decl | owned | 21.97M | 56.08M | 25.92M | 0.39 → **0.85** | -53.77% |
| dtd-decl | reader | 23.59M | 55.85M | 26.10M | 0.42 → **0.90** | -53.27% |
| dtd-decl | push | 23.97M | 59.00M | 29.55M | 0.41 → **0.81** | -49.92% |
| dtd-decl | buffered | 24.04M | 59.00M | 29.61M | 0.41 → **0.81** | -49.82% |
| dtd-refs | dom | 53.77M | 143.29M | 77.12M | 0.38 → **0.70** | -46.18% |
| dtd-refs | owned | 53.77M | 143.42M | 74.72M | 0.37 → **0.72** | -47.90% |
| dtd-refs | reader | 49.56M | 129.52M | 60.78M | 0.38 → **0.82** | -53.07% |
| dtd-refs | push | 51.87M | 148.87M | 54.49M | 0.35 → **0.95** | -63.40% |
| dtd-refs | buffered | 49.64M | 168.97M | 118.18M | 0.29 → **0.42** | -30.06% |
| dtd-nested | dom | 49.93M | 237.10M | 109.96M | 0.21 → **0.45** | -53.62% |
| dtd-nested | owned | 49.93M | 245.83M | 110.36M | 0.20 → **0.45** | -55.11% |
| dtd-nested | reader | 55.77M | 222.16M | 86.23M | 0.25 → **0.65** | -61.18% |
| dtd-nested | push | 106.24M | 239.46M | 83.06M | 0.44 → **1.28** | -65.31% |
| dtd-nested | buffered | 56.00M | 257.50M | 141.42M | 0.22 → **0.40** | -45.08% |
| dtd-attrs | dom | 50.40M | 84.90M | 38.51M | 0.59 → **1.31** | -54.64% |
| dtd-attrs | owned | 50.40M | 92.56M | 46.08M | 0.54 → **1.09** | -50.22% |
| dtd-attrs | reader | 40.90M | 77.44M | 30.95M | 0.53 → **1.32** | -60.04% |
| dtd-attrs | push | 25.21M | 83.35M | 29.68M | 0.30 → **0.85** | -64.40% |
| dtd-attrs | buffered | 40.74M | 89.12M | 49.01M | 0.46 → **0.83** | -45.01% |
| dtd | dom | 120.01M | 283.54M | 160.38M | 0.42 → **0.75** | -43.44% |
| dtd | owned | 120.01M | 296.41M | 173.88M | 0.40 → **0.69** | -41.34% |
| dtd | reader | 108.70M | 267.08M | 143.40M | 0.41 → **0.76** | -46.31% |
| dtd | push | 87.99M | 292.89M | 113.80M | 0.30 → **0.77** | -61.15% |
| dtd | buffered | 108.88M | 319.60M | 207.96M | 0.34 → **0.52** | -34.93% |

Timing with the matched driver (9 alternating pairs, 20 documents per
sample, chunk 8192, parse + observe + destruction; ratio of medians
libxml2/yaxml with the per-pair min–max), main → after, same host
(Apple M1 Max, other jobs running):

| Fixture | DOM | Owned | Reader | Push | Buffered |
| --- | ---: | ---: | ---: | ---: | ---: |
| dtd-decl | 0.61 → **1.26** (1.25-1.27) | 0.61 → **1.25** (1.23-1.26) | 0.68 → **1.39** (1.35-1.42) | 0.65 → **1.25** (1.20-1.34) | 0.65 → **1.26** (1.19-1.27) |
| dtd-refs | 0.36 → **0.61** (0.61-0.62) | 0.38 → **0.66** (0.64-0.67) | 0.37 → **0.74** (0.74-0.75) | 0.32 → **0.92** (0.90-0.94) | 0.28 → **0.40** (0.40-0.40) |
| dtd-nested | 0.23 → **0.44** (0.43-0.44) | 0.22 → **0.45** (0.44-0.46) | 0.26 → **0.61** (0.60-0.64) | 0.41 → **1.34** (1.32-1.36) | 0.21 → **0.40** (0.39-0.41) |
| dtd-attrs | 0.53 → **1.16** (1.15-1.20) | 0.50 → **1.07** (1.05-1.08) | 0.50 → **1.27** (1.26-1.29) | 0.32 → **0.88** (0.87-0.89) | 0.45 → **0.88** (0.87-0.95) |
| dtd | 0.38 → **0.66** (0.65-0.66) | 0.37 → **0.64** (0.63-0.64) | 0.37 → **0.72** (0.71-0.74) | 0.27 → **0.78** (0.78-0.80) | 0.31 → **0.50** (0.50-0.52) |

DTD-free rows did not move: `attributes`, `namespaces`, `flat` and `mixed`
in DOM, reader, push and buffered modes stay within ±1.6% instructions of
main (largest: `attributes` DOM/reader +1.5%, the one start-tag path now
also serving DTD tags).

Root causes, in order of weight:

1. A second, copying token pipeline. Every content token after a DOCTYPE
   and every token of every entity expansion was re-tokenized into owned
   storage (`tokenizer/local.rs`, `process_dtd_*_owned`), then published
   through owned events. Stable input (slices, the DOMs) and replacement
   text are now scanned in place by `content::scan`; push reads plain UTF-8
   chunks in place as the DTD-free stream does; replacement tokens go
   through the physical token path with a `replacement` flag (its markup
   errors stop the parse where physical input recovers).
2. Per-reference rescanning. Each reference re-tokenized its entity's
   replacement text. The text is now scanned once into cached pieces
   (charged to catalog storage); every reference still charges its whole
   expansion to the amplification and expansion ledgers, so
   `max_entity_amplification`, `max_entity_expansion_bytes` and the
   per-use cost asserted by `tests/amplification.rs` are unchanged.
3. A separate DTD start-tag path (`RawTag` → `PreparedTag` → pending
   namespaces → publish) with its own name copies and hash sets. DTD tags
   now share `start_element` with a `TagDeclarations` view: one element
   declaration lookup per tag, unchanged attribute values borrowed,
   duplicate checks linear only below 8 attributes.
4. Hashing. Entity and element lookups went through SipHash per reference
   and per tag. `Recent<T>` puts a 64-slot direct-mapped cache in front of
   the SipHash maps; ATTLISTs are searched up to 8 attributes and indexed
   beyond; the declared map keeps only unbound names.

Security of the hashing change: std's keyed SipHash (`RandomState`) stays
the backing store; no unkeyed hash map was introduced and no dependency
added. `Recent` slots are chosen by an unkeyed hash, so an attacker can
choose names that collide, but a collision only evicts a slot: the lookup
then falls back to the keyed table, so the worst case is the cost before
this change plus one slot comparison of at most 15 bytes. Linear searches
are bounded by constants (8 ATTLIST entries, 8 attributes per tag before
hashing; specified-attribute bookkeeping past 64 declarations is sorted
and binary-searched). A tag now charges copied defaults as the element's
precomputed total less its specified attributes, instead of walking every
declaration twice: `adversarial_scaling default_attrs` (1,000 defaults,
`<a/>` repeated, 4 MiB) went from 2.5 s to 0.07–0.11 s with charges
unchanged (`tests/amplification.rs` `only_unspecified_defaults_are_charged`
passes on main too). Every other adversarial family keeps its slope (DOM
and reader, 1 and 4 MiB); `ids`, `entity_refs`, `many_entities`,
`mixed_valid` and `req_attrs` are 1.5–3.8x faster. One got slower:
`pe_chain` (1.4 M `%p;` references to a comment in the internal subset)
retires 5.5% more instructions (5.47G → 5.78G at 4 MiB, 0.28 → 0.31 s),
spread over the DTD scanner's run handling with no single hot spot.

By time yaxml leads in 9 of 25 rows (all of `dtd-decl`, `dtd-attrs` DOM,
owned and reader, `dtd-nested` push) against none before; by instructions in
4. `dtd-decl` runs faster than libxml2 while retiring more instructions
(libxml2's declaration phase is allocation-bound).

Remaining gaps: `buffered` (0.40–0.88) pays the session's per-event
lend/prepare/drive layering; lending from `fill_buf` would break the
contract that `position()` equals consumed source bytes and gained under
2% when tried. DOM and owned `dtd-nested` (0.44–0.45) and `dtd-refs` (0.61–0.72)
pay the builders on top of the reader and still re-parse each reference
where libxml2 copies the entity's cached node list. Push `dtd-attrs`
(0.85–0.88) is libxml2's cheapest mode (SAX without a tree: 25M instructions
against 50M for its DOM); not profiled further. The declaration phase
(`dtd-decl`, 0.81–0.90 by instructions) still allocates per declaration.

### Fifth series (2026-10-19, `840662a` → `11608692`)

Ratios are **libxml2 median / yaxml median** of parse plus destruction per
document, main after recovery round two (`840662a`) → the fifth series
(`11608692`), with the min–max of the per-pair ratios after. Method as in
the fourth series: release `parse_bench` of both revisions and the static
libxml2 2.15.4 helper under `parse_benchmark`, 9 alternating pairs per
workload with both revisions back to back; every small-chunk row, `flat`,
`unicode`, `deep` and `normalized`, `tiny` reader and the noisier `mixed`
rows were repeated with 21 pairs, which the table shows. Apple M1 Max shared
with other jobs (load average 10-30, so single-pair spreads are wide).
yaxml leads in **63 of 65** workloads (61 before). Instruction counts below
are per document (difference of N- and 2N-document runs, minimum of 3).

| Fixture | DOM | Owned | Reader | Push | Buffered |
| --- | ---: | ---: | ---: | ---: | ---: |
| tiny | 3.62 → **3.43** (3.35–3.60) | 2.27 → **2.17** (2.10–2.28) | 4.09 → **3.73** (3.59–3.85) | 1.49 → **1.50** (1.39–1.52) | 2.19 → **2.13** (2.06–2.20) |
| flat | 1.30 → **1.33** (1.32–1.37) | 1.14 → **1.17** (1.15–1.19) | 1.36 → **1.42** (1.39–1.46) | 0.99 → **1.02** (0.72–1.14) | 1.03 → **1.03** (1.02–1.04) |
| text | 2.96 → **2.95** (2.88–2.99) | 2.58 → **2.59** (2.51–2.67) | 1.29 → **1.28** (1.27–1.29) | 1.14 → **1.14** (1.14–1.14) | 1.27 → **1.27** (1.26–1.28) |
| attributes | 2.10 → **2.10** (2.08–2.24) | 1.75 → **1.72** (1.63–1.76) | 1.41 → **1.41** (1.38–1.44) | 1.05 → **1.08** (1.07–1.09) | 1.38 → **1.38** (1.36–1.39) |
| namespaces | 1.59 → **1.60** (1.57–1.65) | 1.32 → **1.34** (1.32–1.35) | 1.92 → **1.91** (1.85–1.96) | 1.09 → **1.12** (1.10–1.16) | 1.44 → **1.44** (1.42–1.46) |
| normalized | 1.61 → **1.62** (1.58–1.68) | 1.47 → **1.54** (1.43–1.60) | 1.76 → **1.79** (1.69–1.84) | 1.11 → **1.21** (1.16–1.26) | 1.40 → **1.46** (1.42–1.50) |
| unicode | 1.39 → **1.40** (1.33–1.53) | 1.20 → **1.21** (1.20–1.27) | 1.65 → **1.65** (1.64–1.74) | 1.02 → **1.05** (1.00–1.10) | 1.27 → **1.27** (1.20–1.34) |
| deep | 1.19 → **1.15** (1.06–1.22) | 1.03 → **1.06** (1.00–1.13) | 1.90 → **1.72** (1.62–1.87) | 1.03 → **1.05** (0.98–1.12) | 1.22 → **1.21** (1.15–1.28) |
| mixed | 1.94 → **2.20** (2.16–2.26) | 1.62 → **1.80** (1.76–1.90) | 2.39 → **2.63** (2.61–2.66) | 1.09 → **1.23** (1.16–1.30) | 1.55 → **1.64** (1.62–1.75) |

| Small chunks | Push 1 | Push 7 | Buffered 1 | Buffered 7 |
| --- | ---: | ---: | ---: | ---: |
| tiny | 1.90 → **1.89** (1.35–2.26) | 1.45 → **1.56** (1.03–2.18) | 2.64 → **2.30** (1.87–2.93) | 2.36 → **2.09** (1.14–2.47) |
| text | 1.20 → **1.19** (0.75–1.39) | 1.17 → **1.18** (1.09–1.26) | 2.23 → **2.20** (1.92–2.32) | 1.66 → **1.66** (1.64–1.68) |
| namespaces | 1.12 → **1.19** (0.96–1.92) | 0.96 → **0.99** (0.85–1.08) | 1.98 → **1.95** (1.92–1.99) | 1.55 → **1.54** (1.52–1.57) |
| normalized | 1.22 → **1.26** (1.19–1.27) | 0.99 → **1.06** (1.03–1.09) | 2.16 → **2.17** (2.16–2.19) | 1.59 → **1.65** (1.63–1.68) |
| mixed | 1.17 → **1.21** (1.17–1.28) | 0.89 → **0.92** (0.91–0.97) | 2.24 → **2.27** (2.22–2.33) | 1.67 → **1.73** (1.69–1.81) |

Retired instructions per document (millions), libxml2, main before recovery
round two (`c4c34a7`), main after it (`840662a`) and this series. `comments`
and `cdata` are synthetic: `<r>` and 4,000 comments or CDATA sections of
about 30 bytes, each followed by a newline.

| Workload | libxml2 | before recovery | main | after |
| --- | ---: | ---: | ---: | ---: |
| comments reader | 13.43 | 4.68 | 5.32 | 4.70 |
| comments DOM | 12.22 | 5.78 | 6.46 | 5.65 |
| cdata reader | 23.28 | 4.34 | 4.99 | 4.36 |
| cdata DOM | 19.32 | 5.44 | 6.13 | 5.35 |
| mixed reader | 20.75 | 8.02 | 8.62 | 8.06 |
| mixed DOM | 20.99 | 10.13 | 10.69 | 9.97 |
| flat push 8192 | 11.13 | 11.66 | 11.63 | 11.38 |
| unicode push 8192 | 22.13 | 23.11 | 23.09 | 22.68 |
| deep push 8192 | 0.18 | 0.20 | 0.20 | 0.21 |
| normalized push 8192 | 19.10 | 19.70 | 18.59 | 16.99 |
| mixed push 8192 | 9.48 | 8.79 | 8.67 | 7.69 |
| mixed push 7 | 14.82 | 17.49 | 17.43 | 16.09 |
| namespaces push 7 | 41.08 | 41.50 | 41.56 | 41.12 |
| normalized push 7 | 27.27 | 31.30 | 30.24 | 28.50 |
| comments push 7 | 14.45 | 15.67 | 15.71 | 15.04 |

Root causes, from exact per-call instruction traces (method below):

- **Recovery checks in strict code.** Recovery round two added per-token
  checks to the slice reader and the shared token parser (undeclared
  references, rewinds, character repair). Their branches were cheap; the
  cost (comment-only reader +13%, CDATA +15%, mixed +7%) was codegen: a
  cold call kept `normalized`'s arguments in callee-saved registers, the
  rewind kept `start` and the input live across `process_token`, and
  `delimited_body` fell out of line, so its prefix and suffix checks became
  `memcmp` calls (the "unexplained" comment-only cost, about 18 of the 80
  instructions per event). Cold helpers alone did not help: LLVM spilled
  elsewhere. The slice reader now dispatches once per event on
  `ParserState::rereads` to a `REREADS` instantiation of its token loop and
  of `read_token`, so strict reading and the streaming, DTD and writer
  frontends compile without any recovery check; the no-change normalization
  scan is inlined at each value and the changing path is out of line.
- **By-value delivery.** `PushParser` callbacks now take `&Event`: the
  event is built once and lent, the attribute list stays owned by the parser
  instead of being rebuilt as a borrowed one, and `lend_event` is inlined
  into each delivery loop (`flat` push -2.2%). `SourceOrigin` stays a 56-byte
  `Copy` value; it costs nothing when the callback ignores it.
- **An allocation per normalized value.** Streaming normalized text, comment,
  CDATA, PI and attribute values into a `String` each, freed after the
  callback; malloc and free were about 35% of `normalized` push. While events
  are lent the values share one reused buffer (normalization never lengthens
  a value, so it stays within the token size) and are lent from it, and
  normalization appends through a local buffer (`normalized` push -8.6%,
  `mixed` push -11%).
- **Short-token character screen.** `invalid_char` folded even 8-byte
  tokens as a 64-byte block; tokens under 16 bytes now use a table scan
  (`mixed` push/7 -1.9%).

Tried and rejected: tail-calling the repair path from `normalized` (+8
instructions per value), the table scan up to 64 bytes (2.3x slower at 40
bytes) and a SWAR byte screen.

Remaining losses are push/7. `mixed` (0.92) retires 8.6% more instructions
than libxml2: per 7-byte chunk about 285 in the tokenizer carry path
(`advance_inner`, `step`, `extend`, prefix classification) and about 100
validating each completed token (`str::from_utf8`, which safe code cannot
skip, plus the character screen). `namespaces` push/7 (0.99) is at parity
in instructions. Two slice-reader rows lost time without retiring more
instructions: `tiny` reader (4.09 → 3.73, +4.5% instructions, about 40 ns
per document) and `deep` reader (1.90 → 1.72, +1.8%); both appear with the
`&Event` and value-lending commit (`dd6bf1b`), which grew `ParserState` from
448 to 496 bytes. The `tiny` loss was that size: each slice-reader
construction moves the state (a 1.2 KB `Reader` is copied through
`configured` and the caller's `Result`), and boxing the lent-value storage
on first use (456 bytes) gives `tiny` reader 4.5% fewer instructions and
10% less time than main, 4% less than before the series (21 pairs). A
24-byte `Vec` in its place (472 bytes) recovered the time too but cost
push 0.3-1.4% instructions through the extra indirection. `deep` reader
stays about 3% slower than before the series while retiring 0.7% fewer
instructions; it is code layout, not size.

### Fourth series (2026-10-19, `2a0e872` → `45c756e`)

Ratios are **libxml2 median / yaxml median** of parse plus destruction per
document (above 1 favors yaxml), before → after the fourth optimization
series (`2a0e872` → `45c756e`, which includes a merge of main), with the
min–max of the per-pair ratios after. Method:
release `parse_bench` of each revision and the static libxml2 2.15.4 helper
under `parse_benchmark`, one calibration targeting 50 ms batches, 9
alternating pairs per workload with both revisions back to back; the
noisiest rows (`flat`, `unicode`, `deep` push, `deep` owned and every
1/7-byte push) were repeated with 21 pairs, which the table shows. Apple M1
Max shared with other jobs (load average 5-30). yaxml leads in **61 of 65**
workloads (58 for the baseline in the same runs). Retired instructions per
document (difference of N- and 2N-document runs) rose by at most 1.2% on
any workload; small-chunk buffered input fell 10-20%.

| Fixture | DOM | Owned | Reader | Push | Buffered |
| --- | ---: | ---: | ---: | ---: | ---: |
| tiny | 3.86 → **3.91** (3.70–4.03) | 2.43 → **2.40** (2.33–2.46) | 4.36 → **4.34** (4.31–4.40) | 1.47 → **1.51** (1.46–1.57) | 2.19 → **2.19** (2.13–2.31) |
| flat | 1.40 → **1.34** (1.31–1.37) | 1.19 → **1.22** (1.11–1.50) | 1.39 → **1.38** (1.37–1.55) | 0.89 → **0.95** (0.95–0.97) | 1.10 → **1.03** (0.82–1.12) |
| text | 2.90 → **2.83** (2.82–3.05) | 2.50 → **2.53** (1.33–3.35) | 1.29 → **1.29** (1.27–1.30) | 1.14 → **1.14** (1.11–1.15) | 1.27 → **1.27** (1.26–1.28) |
| attributes | 2.08 → **2.10** (2.05–2.12) | 1.74 → **1.72** (1.70–1.73) | 1.41 → **1.42** (1.38–1.43) | 1.06 → **1.06** (1.05–1.07) | 1.36 → **1.38** (1.37–1.39) |
| namespaces | 1.62 → **1.63** (1.53–1.64) | 1.32 → **1.31** (1.29–1.33) | 1.91 → **1.93** (1.90–1.99) | 1.08 → **1.09** (1.08–1.10) | 1.42 → **1.45** (1.44–1.49) |
| normalized | 1.60 → **1.59** (1.56–1.65) | 1.47 → **1.48** (1.41–1.73) | 1.73 → **1.71** (1.56–1.78) | 1.04 → **1.08** (1.06–1.26) | 1.35 → **1.39** (1.36–1.40) |
| unicode | 1.38 → **1.39** (1.37–1.54) | 1.22 → **1.23** (1.21–1.26) | 1.65 → **1.67** (1.64–1.70) | 0.99 → **1.02** (1.01–1.04) | 1.27 → **1.28** (1.25–1.28) |
| deep | 1.18 → **1.20** (1.19–1.24) | 1.01 → **1.01** (0.98–1.04) | 1.81 → **1.83** (1.78–1.88) | 0.98 → **1.04** (0.99–1.06) | 1.20 → **1.23** (1.21–1.26) |
| mixed | 2.13 → **2.13** (2.08–2.17) | 1.71 → **1.79** (1.67–1.83) | 2.64 → **2.64** (2.60–2.68) | 1.05 → **1.08** (1.07–1.24) | 1.52 → **1.56** (1.55–1.59) |

| Small chunks | Push 1 | Push 7 | Buffered 1 | Buffered 7 |
| --- | ---: | ---: | ---: | ---: |
| tiny | 1.78 → **1.89** (1.81–2.00) | 1.39 → **1.42** (1.36–1.50) | 2.48 → **2.62** (2.58–2.68) | 2.12 → **2.10** (2.02–2.16) |
| text | 1.02 → **1.19** (1.18–1.20) | 0.97 → **1.19** (1.16–1.21) | 1.76 → **2.24** (2.23–2.25) | 1.43 → **1.67** (1.65–1.70) |
| namespaces | 1.03 → **1.12** (1.07–1.13) | 0.92 → **0.98** (0.92–1.03) | 1.74 → **1.97** (1.95–2.00) | 1.40 → **1.54** (1.53–1.56) |
| normalized | 1.08 → **1.20** (1.18–1.21) | 0.90 → **0.97** (0.95–1.00) | 1.86 → **2.16** (2.14–2.18) | 1.47 → **1.57** (1.55–1.61) |
| mixed | 1.02 → **1.18** (1.15–1.21) | 0.79 → **0.89** (0.87–0.91) | 1.95 → **2.27** (2.24–3.19) | 1.50 → **1.71** (1.67–2.26) |

Retired instructions per document (millions) for the rows that lost before:

| Workload | libxml2 | before | after |
| --- | ---: | ---: | ---: |
| flat push 8192 | 11.15 | 11.94 | 11.61 |
| unicode push 8192 | 22.07 | 23.37 | 23.07 |
| deep push 8192 | 0.18 | 0.21 | 0.20 |
| mixed push 7 | 14.84 | 20.28 | 17.53 |
| namespaces push 7 | 41.16 | 44.50 | 41.58 |
| normalized push 7 | 27.10 | 33.90 | 31.36 |
| text push 7 | 27.13 | 29.36 | 24.44 |

Root causes, found with exact per-call instruction traces: a throwaway
example calls `PushParser::feed` from an `#[inline(never)]` function, lldb
single-steps 20-40 of those calls, and `atos -i` attributes every retired
instruction through its inline chain.

- **Call boundaries per chunk.** `Stream::advance` and `Tokenizer::advance`
  were out-of-line calls, entered once or twice per small chunk, with
  prologues saving most callee-saved registers. Inlining them removed 5% of
  `mixed` and 9% of `text` push/7 instructions.
- **Byte-wise prefix classification.** A markup prefix still took one
  tokenizer step and one buffer append per byte (nine for `<![CDATA[`). It
  now classifies every available byte in one step (`mixed` push/7 -4%).
- **Checked appends.** Every token run went through the out-of-line growth
  path even with spare capacity. Runs within capacity now append directly
  (`text` push/7 -8%, `mixed`, `namespaces`, `normalized` push/7 -3 to -5%).
- **Duplicate state in `PushParser`.** It kept its own sticky failure,
  finished flag and input count next to the session's identical ones and
  checked both per feed. The session now owns all three (about 1%).
- **Event copies.** `deliver` and `parse_valid` were out of line, so the
  88-byte `Event` was copied across them; inlining them removed 3% of `flat`
  and 4% of `deep` push instructions.

Tried and rejected, by the same counts: parsing complete tokens in place in
chunk remainders under 32 bytes (+6% on push/7: short tails rarely hold a
whole token, so the extra scan is wasted), scalar loops instead of `memchr`
on short haystacks (+2%), and `#[inline(always)]` on `process_token` (-2%
instructions but +6-11% cycles on `flat` and `mixed` push).

Event delivery timing is unchanged: every event is still delivered in the
feed call that supplies its last byte.

Remaining losses are all push. `flat` (0.95) spends about 50 instructions
per event in `deliver` copying and rebuilding the by-value `Event` and
`SourceOrigin` (88 and 56 bytes) and about 15 dropping it after the
callback; libxml2 passes pointers. Removing that needs a delivery shape
that does not move events, which the public callback signature fixes.
`mixed` push/7 (0.89) still retires 18% more instructions than libxml2:
per 7-byte chunk about 400 in the tokenizer carry path, 90 validating the
completed token (`from_utf8` plus the XML-character screen on 5-10 bytes)
and 70 in shared text normalization. `namespaces` and `normalized` push/7
(0.97) are within 3% on time but `normalized` retires 16% more
instructions, mostly attribute and text normalization shared with every
mode.

### Third series (2026-10-19, `44531bc` → `53f3f19`)

Root causes found by sampling (`xctrace` Time Profiler with line tables,
attributed through inline chains) and instruction counts:

- **Byte-wise delimiter states.** The incremental tokenizer classified markup
  prefixes and comment/CDATA/PI terminators one byte per call, so a token cut by
  a small chunk paid a call per delimiter byte. It now takes one memchr-driven
  step per lexical state, carrying only the trailing marker run (`mixed` push/7
  -9% instructions). Short inputs no longer attempt whole-token adoption that
  the scan then repeated, and plain UTF-8 bypasses the decoder closure.
- **Per-call substring searchers.** `memmem::find` built a searcher for every
  comment, CDATA, PI and `]]>` check; a memchr-led search is 6-7% fewer
  instructions on `mixed` in every mode.
- **Per-name URI copies.** Streaming events copied each namespaced name's URI
  into a fresh `String` (a malloc/free pair per element and attribute, about a
  tenth of `unicode` and `namespaces` push). Events now lend the parser's
  retained URIs for the delivery borrow.
- **Regrowth and empty lookups.** Element stacks doubled from four entries on
  every fresh parser, and unprefixed names searched an empty binding stack
  (`deep` push -6% instructions).

## Buffered pull observations

`cargo bench --bench pull` runs tiny elements, attributes, text, lexical payloads,
normalization and namespace scopes in UTF-8 and both UTF-16 orders. Source bytes
are constructed before timing. Each sample creates a fresh reader over a caller
`BufReader<Cursor<_>>`, consumes every event to EOF and destroys the reader inside
the timed batch. It observes names, namespace fields, attributes and payload
slices without formatting them. Exact source-range handling is extra yaxml work.

Output records encoded bytes, siblings, buffer capacity, iterations, event count,
observed logical byte lengths, elapsed nanoseconds, summed first-event latency
and summed parser-consumed bytes when that first event returns. Divide summed
latency/consumption by iterations. First-event latency includes source/reader
construction. Parser consumption excludes bytes the caller's BufReader prefetched.
Every throughput sample asserts that parsing reached the complete input length.
The timer includes the per-iteration first-event measurement overhead; do not
compare its throughput directly with an uninstrumented runner.

The default release matrix uses sibling counts 1/256/4096 and capacities
1/2/3/7/64/8192/65536. Its batch size targets roughly one MiB, capped at 50
iterations. Debug smoke uses counts 1/16 and one iteration. Each of the following
environment variables selects a single value instead of its matrix axis:

| Variable | Values |
| --- | --- |
| `YAXML_PULL_FIXTURE` | `tiny`, `attributes`, `text`, `lexical`, `normalized`, `namespaces` |
| `YAXML_PULL_SIBLINGS` | Positive sibling count |
| `YAXML_PULL_CAPACITY` | Positive source-buffer capacity |
| `YAXML_PULL_ENCODING` | `utf8`, `utf16le`, `utf16be` |
| `YAXML_PULL_ITERATIONS` | Positive batch size |
| `YAXML_PULL_PAYLOAD_BYTES` | Text/comment/CDATA/PI payload length, default 16 |

Select all axes for one sample per fresh process when collecting resident-memory
series. Record the exact executable path from Cargo's build output; do not time
Cargo compilation. A fixed token size, depth and live namespace scope should
produce a retained-state plateau as sibling count grows. Public observables
cannot identify tokenizer capacity or live namespace/structural entries: that
acceptance gate requires the private retained-state tests or a documented
fresh-process memory series. Caller buffer, input Vec, allocator caches and
process baseline contribute to RSS and must be distinguished. The cumulative
owned-string budget is not a measurement of live memory.

The default matrix has short tokens. Select a larger `YAXML_PULL_PAYLOAD_BYTES`
with one sibling for token-size series; the lexical fixture applies that length
to each comment, CDATA and PI payload. The runner records `status=rejected` and
the error if a finite policy ceiling stops a sample, omitting its throughput.
Namespace URI/depth/attribute axes remain separate experiments. Do not silently
drop a rejected sample or call an unconsumed input a throughput measurement.

## Complete-document serialization

`cargo bench --bench serialize` measures `Writer::write_document` and
`write_owned_document` on trees constructed before timing. Each iteration
constructs a fresh writer and sink, writes the complete document, performs its
final flush and destroys the writer/sink inside the timed batch. Parsing and tree
destruction are excluded. The counter sink observes every output slice without
retaining it; the buffered sink wraps the same counter in an 8 KiB `BufWriter`.
Each successful iteration verifies confirmed output bytes and one underlying
flush. Output reports shape, tree form, sink, source bytes, siblings, depth,
iterations, total output bytes, underlying writes/flushes, elapsed nanoseconds
and output MiB/s. Buffering changes sink-call overhead; neither sink measures
filesystem or network performance.

The release matrix uses sibling counts 1/256/4096, both tree forms and both sink
forms. Debug smoke uses 1/16 and one iteration. Default batches target about one
MiB of source input, capped at 100 iterations. These selectors reduce an axis to
one value for controlled series:

| Variable | Values |
| --- | --- |
| `YAXML_SERIALIZE_SHAPE` | `flat`, `text`, `normalized`, `lexical`, `namespaces` |
| `YAXML_SERIALIZE_TREE` | `borrowed`, `owned` |
| `YAXML_SERIALIZE_SINK` | `counting`, `buffered` |
| `YAXML_SERIALIZE_SIBLINGS` | Positive sibling count |
| `YAXML_SERIALIZE_DEPTH` | Positive enclosing element depth, default 1 |
| `YAXML_SERIALIZE_ATTRIBUTES` | Attributes per namespace fixture child, default 1 |
| `YAXML_SERIALIZE_PAYLOAD_BYTES` | Text/comment/CDATA/PI payload length, default 32 |
| `YAXML_SERIALIZE_URI_BYTES` | Namespace URI suffix length, default 16 |
| `YAXML_SERIALIZE_ITERATIONS` | Positive batch size |

Use independent sibling, depth, attribute, URI and token-length series. Writer
and parse depth/attribute ceilings follow requested dimensions; other finite
budgets remain defaults. Rejected samples report `status=rejected` or
`status=parse_rejected` and an error, without throughput. Do not drop these
samples. UTF-16 output is outside this API; owning UTF-16 input conversion is
tested separately and would occur before the serialization clock.

For RSS measurements select a single tree/sink/shape and run the built binary in
a fresh process. The selected tree and its source input remain live outside the
clock. Their memory, the sink buffer, allocator caches and process baseline
contribute to RSS; a whole-process reading does not isolate serializer scratch.
Private traversal/allocation tests remain necessary to establish stackless
traversal and reusable metadata capacity. The cumulative string quota does not
measure live retained bytes. No memory or speed improvement is established by
this runner alone.

A fair libxml2 2.15.4 serialization comparison needs a separately built
`xmlSaveDoc` harness with trees prepared outside timing, matched UTF-8 output,
declaration/empty-element/formatting policy, counting or equivalently buffered
callback sinks, final flush, context construction/destruction and output-byte
verification. Namespace repair and richer libxml2 output options change the work.
Validate semantic equivalence separately and report unavoidable differences;
do not compare this batch timer with whole-process xmllint startup. The pinned
[save implementation](https://github.com/GNOME/libxml2/blob/v2.15.4/xmlsave.c)
defines the reference lifecycle. No matched libxml2 timing harness is supplied.

## Owned-value editing

`cargo bench --bench mutation` reparses a fresh owned tree for each iteration,
because edits consume cumulative capacity. Fixture generation and UTF-8/UTF-16
encoding are outside all clocks. It reports separate times for parse/handle
setup, Editor construction and edits, final value checks plus serialization/
complete reparse/checksum, and destruction. The edit clock includes selector
dispatch and handle selection. Documents and validation buffers are destroyed
after that clock. No document clone or parse is hidden in edit throughput.

Six cases cover repeated/distributed leaf updates, first/middle/last attribute
lookup and alternating large/short values. Release defaults use widths 1/256,
depths 1/64, 32 attributes, five fresh documents and 10,000 operations each;
debug smoke uses widths 1/16, depths 1/4, four attributes, one document and 32
operations. Both matrices cover UTF-8/UTF-16LE/UTF-16BE construction: 72 rows.
Encoding conversion affects setup; the editable strings are always UTF-8.

| Variable | Values |
| --- | --- |
| `YAXML_MUTATION_CASE` | `same_leaf`, `many_leaves`, `attribute_first`, `attribute_middle`, `attribute_last`, `growth_shrink` |
| `YAXML_MUTATION_ENCODING` | `utf8`, `utf16le`, `utf16be` |
| `YAXML_MUTATION_WIDTH`, `YAXML_MUTATION_DEPTH` | One positive size instead of that matrix axis |
| `YAXML_MUTATION_ATTRIBUTES` | Positive attribute count for lookup cases |
| `YAXML_MUTATION_ITERATIONS`, `YAXML_MUTATION_OPERATIONS` | Positive fresh-document and per-document operation counts |
| `YAXML_MUTATION_PAYLOAD_BYTES` | Ordinary/short value bytes, default 32; zero is allowed |
| `YAXML_MUTATION_GROWTH_BYTES` | Large value bytes, default 1,024; at least the short size for growth/shrink |
| `YAXML_MUTATION_MAX_VALUE_BYTES`, `YAXML_MUTATION_MAX_OWNED_BYTES` | `EditLimits` ceilings, including zero for rejection studies |

Rows record dimensions, ceilings, requested/completed operations, phase times,
serialized bytes and an FNV checksum. A rejected edit stops that iteration,
records its error, and validates the successful prefix. Its elapsed time
includes the rejected attempt while throughput counts successful operations;
do not compare such a row as complete-work throughput. Input parse rejection is
reported separately. Serialization/reparse must pass: their depth and attribute
ceilings follow the dimensions, but other byte/token budgets remain defaults.
Oversized custom workloads can therefore terminate during validation.

Use independently varied attribute-count, width, depth and payload series, with
fixed operations and repeated alternating runs. These are yaxml measurements;
no matched libxml2 editing harness, allocation profile or memory advantage is
established. Final milestone results belong in the
[mutation validation report](archive/validation-owned-mutation.md).

## Owned structural editing

`cargo bench --bench structural_mutation` measures fresh leaf insertion,
permanent subtree removal and tombstone-heavy churn. Fixture generation and
UTF-8/UTF-16 transport encoding occur before all clocks. Each row separately
reports parse/handle setup, structural edit calls, complete live traversal,
counting-sink serialization, semantic validation plus serialization/reparse,
and owned-document destruction. Traversal touches public kinds, provenance,
names, values and attributes; churn appends and removes one fresh Text node per
unit so the final live tree stays small while permanent slots accumulate.

The cases are `append_text`, `append_cdata`, `append_comment`, `append_pi`,
`remove_first`, `remove_middle`, `remove_last`, `remove_deep` and `churn`.
Removal selection is prepared outside the edit clock. Shallow removals delete
complete sibling subtrees beneath a depth-selected parent; deep removal deletes
nested branches. Every final state checks live/stale handles, stable fresh IDs,
logical kinds and values, exact live traversal counts, serialization byte
counts and a complete reparse.

| Variable | Meaning |
| --- | --- |
| `YAXML_STRUCTURE_CASE`, `YAXML_STRUCTURE_ENCODING` | Select one case or UTF-8/UTF-16LE/UTF-16BE input |
| `YAXML_STRUCTURE_WIDTH`, `YAXML_STRUCTURE_DEPTH` | Select one positive sibling/ancestor or branch-depth dimension |
| `YAXML_STRUCTURE_PAYLOAD_BYTES` | Select one nonnegative payload size |
| `YAXML_STRUCTURE_ITERATIONS`, `YAXML_STRUCTURE_OPERATIONS` | Fresh documents and edit units per document |
| `YAXML_STRUCTURE_TRAVERSAL_PASSES`, `YAXML_STRUCTURE_SERIALIZATION_PASSES` | Repeated post-edit public-view and counting-sink passes |
| `YAXML_STRUCTURE_MAX_VALUE_BYTES`, `YAXML_STRUCTURE_MAX_OWNED_BYTES`, `YAXML_STRUCTURE_MAX_CREATED_NODES` | Public edit ceilings, including deliberate rejection studies |

Debug defaults keep the all-target smoke execution bounded at width/depth four,
payloads zero/eight, one document and four units. Release defaults cover widths
16/256, depths 1/64, payloads 32/1,024, all three encodings, five documents and
1,000 units; select rows for repeated measurements rather than averaging that
entire matrix. Output uses `key=value` fields with schema version, requested and
completed edit calls, completed units, created/removed/live node counts,
expected permanent slots, traversal visits, serialized bytes, sink calls,
limits, phase nanoseconds, disposition and checksum.

Each row also emits the exact final-output FNV hash and a `reference_model` for
the matched libxml2 lifecycle. `YAXML_STRUCTURE_EXPORT` writes one framed
comparison request after all timers; export requires explicit case, encoding,
width, depth and payload selectors. The request contains the original encoded
input, payload, dimensions, pass counts and exact final UTF-8 bytes. It contains
no Rust pointers or private arena state.

### Matched libxml2 structural run

`harness/tooling/libxml2_structure_bench.c` is compiled separately and never linked by
Cargo. Build it against the pinned static libxml2 2.15.4 archive:

```console
cc -std=c11 -O3 -DNDEBUG -Wall -Wextra -Werror -pedantic \
  -I/private/tmp/yaxml-oracle-21504/build \
  -I/private/tmp/yaxml-oracle-21504/libxml2-2.15.4/include \
  tooling/libxml2_structure_bench.c \
  /private/tmp/yaxml-oracle-21504/build/libxml2.a -lm -pthread -ldl \
  -o /private/tmp/yaxml-libxml2-structure-bench
```

The safe-Rust driver performs an unrecorded version/fixture preflight, then
alternates two prebuilt executables under the same deadline/output ceiling and
validates every pair before printing it. First run
`cargo bench --bench structural_mutation --no-run --offline --locked` and use
the executable path printed by Cargo:

```console
cargo run --release --offline --locked --example structure_benchmark -- \
  --libxml2-helper /private/tmp/yaxml-libxml2-structure-bench \
  --yaxml-helper /absolute/path/to/structural_mutation-HASH \
  --samples 7 --case churn --encoding utf8 \
  --width 16 --depth 64 --payload-bytes 32 \
  --iterations 5 --operations 1000 \
  --traversal-passes 16 --serialization-passes 4
```

Both implementations parse fresh documents and use the same original bytes,
payload, iteration count and edit-unit count. Setup includes parsing, parent
selection and removal-order bookkeeping. The edit clock includes fresh C node
construction plus `xmlAddChild`; removal and churn include both
`xmlUnlinkNode` and `xmlFreeNode`. Target selection and removed-subtree counting
occur before that clock. C's whole-document initial node-count check occurs
outside every clock, avoiding traversal work where Rust has an O(1) arena
length. Traversal, serialization, exact-byte validation and document
destruction have separate clocks. The driver requires each row to match the
requested dimensions and deterministic operation/node counts, then requires
matching byte, hash and common live-traversal accounting across the pair.

libxml2 coalesces adjacent Text nodes and may free the pointer supplied to
`xmlAddChild`. The C helper never reads that pointer again, records the merged
count and traverses the returned survivor. The driver permits and prominently
reports only this live-node-count difference. Empty PI data is an intentional
lexical difference (`<?bench ?>` versus `<?bench?>`) and the matched driver
rejects that configuration; other empty leaf payloads remain exact checks.

libxml2 frees removed records immediately. yaxml retains tombstone slots for
stable handles and counts cumulative owned bytes and created nodes against
public quotas. libxml2 has neither those quotas nor yaxml source provenance;
caller arrays and target-order work are excluded from edit time on both sides.
The current Rust edit clock starts before allocating its live/stale validation
vectors and includes their pushes; the C edit clock uses scalar counters and
has no equivalent allocation. The C `xmlSave` path also buffers internally,
while the Rust `Writer` calls the counting sink directly, so sink-call counts
and serialization times include different buffering policies. These differences
must accompany results. Common traversal observes node kind, qualified-name and
namespace lengths, direct payload length/first byte and attributes. Rust also
observes source provenance ranges, for which libxml2 has no matching retained
representation. The C helper places C11 compiler fences around each traversal
pass and folds pass checksums without even-pass cancellation, keeping those
observations in the timed loop under optimization. A matched row establishes
its measured lifecycle only; it does not isolate the internal algorithm or
establish a blanket speed or memory win.

## Comparability and reporting

For published measurements retain the revision, production/fixture hashes,
compiler and optimization flags, CPU/OS, allocator, source/buffer ownership,
limits, warm-up policy, batch calibration and all repeated samples. Alternate
before/after or library order; report median and spread per workload. A combined
average must not conceal a slower workload. First-event latency and full-input
throughput answer different questions.

A libxml2 2.15.4 comparison needs a separately built callback-I/O harness with
matched source chunks, semantic options, payload retrieval and fresh-context
lifecycle. Its common observable work is logical names, namespaces, attributes
and values; validate yaxml's exact ranges independently. Libxml2 memory-reader
input copying and xmllint's repeated-context/partial-payload timing loops are
different workloads. No matched external harness or comparative result is added
by this runner. [Pinned reader](https://github.com/GNOME/libxml2/blob/v2.15.4/xmlreader.c)
and [xmllint](https://github.com/GNOME/libxml2/blob/v2.15.4/xmllint.c) define those
reference paths.

## DTD validation

`cargo bench --bench valid` times `Document::validate` (`tree`), reading a
document's `BufferedReader` events (`events`) and reading them through a
`valid::StreamValidator` (`stream`) on two fixtures: `flat` (10,000 items
with element, mixed and `EMPTY` content, `ID`/`IDREF`, enumerated and
`NMTOKENS` attributes, 1.4 MB) and `deep` (200 nested `(r?)` elements).
`YAXML_BENCH_ITERATIONS`, `YAXML_BENCH_FIXTURE` and `YAXML_BENCH_MODE` fix
the run for `/usr/bin/time -l` instruction counts (per iteration = run with N
iterations minus run with 0, divided by N; 20 iterations for `flat`, 2,000
for `deep`). Moving the tree pass onto the event engine (2026-10-19) took
`tree` from 235.17M to 173.20M retired instructions per `flat` iteration
(-26.35%) and from 300,930 to 191,452 per `deep` iteration (-36.38%): no
attribute vectors per element, and each child is stepped through its
parent's automaton during the one walk instead of a second walk per element. `stream` costs 14.72% (`flat`) and 13.58%
(`deep`) over `events`.
