# Parser, serialization and mutation benchmark evidence

Matched parsing and structural runners compare yaxml with pinned libxml2;
other runners measure yaxml alone. Results apply to their recorded workloads
and lifecycles. Build release binaries before recording runs; debug execution
under `cargo test --all-targets` is only a functional smoke check.

## Matched DOM and streaming parsing

The `parse_bench` example and external `tooling/libxml2_parse_bench.c` helper
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
`unicode`, `deep` and `mixed`. Select `--input PATH` instead of `--fixture` for
external UTF-8, XML 1.0, DTD-free bytes. Modes are `dom`, `owned`, `reader`,
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
change, not a tuning pass. `mixed` push/7 retires 6.7% more instructions
than libxml2 (15.82M against 14.82M) for the reasons in the fifth series;
no new lead.

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

`tooling/libxml2_structure_bench.c` is compiled separately and never linked by
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
