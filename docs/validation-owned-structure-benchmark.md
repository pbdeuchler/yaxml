# Owned structural benchmark validation

This report records a yaxml-only directional run of the structural editing
benchmark introduced by commit `7b217d4d7d0e7d64bf6774783da243de5c74986e`
on top of `c0997ba30e37e75e4078b30d56e8c47f6b41b297`. It is not a libxml2
comparison or a performance claim.

## Environment and method

- Date: 2026-10-16
- Host: `aarch64-apple-darwin`, Darwin 27.0.0
- Compiler: `rustc 1.95.0 (59807616e 2026-04-14)`, LLVM 22.1.2
- Cargo: `cargo 1.95.0 (f2d3ce0bd 2026-03-21)`
- Profile: Cargo `bench` release profile, offline and locked
- Dependency state: repository `Cargo.lock`; no benchmark dependency added

The complete release matrix was run with:

```console
cargo bench --bench structural_mutation --offline --locked
```

It produced 216 complete rows: nine cases, two widths, two depths, two payload
sizes and three source encodings. Each row aggregates five independently
parsed and edited documents. Fixture construction and UTF-8/UTF-16 encoding
precede every phase timer. Setup, edit, live traversal, counting-sink
serialization, semantic validation plus byte serialization/reparse, and
`OwnedDocument` destruction have independent clocks.

A focused run repeated each case in seven independent processes using UTF-8,
width 16, depth 64, 32-byte payloads, five documents, 1,000 configured edit
units, 16 traversal passes and four serialization passes. Removal rows complete
16 units per document because only 16 sibling branches exist. Times below are
nanoseconds per process row, with the median followed by `[minimum..maximum]`.

| Case | Setup | Edit | Traversal | Serialization | Validation | Drop |
| --- | ---: | ---: | ---: | ---: | ---: | ---: |
| append text | 93,292 [89,500..98,416] | 1,049,001 [1,045,418..1,064,540] | 434,583 [434,000..442,166] | 3,074,666 [3,045,750..3,123,752] | 1,677,001 [1,643,959..1,723,041] | 499 [417..791] |
| append CDATA | 91,166 [89,748..101,416] | 1,170,792 [1,154,000..1,221,958] | 439,666 [436,500..461,165] | 2,468,749 [2,460,419..2,550,126] | 1,759,458 [1,725,791..1,768,165] | 500 [416..542] |
| append comment | 95,417 [91,624..97,250] | 1,176,001 [1,158,416..1,184,248] | 441,583 [436,458..449,583] | 2,499,833 [2,486,833..2,563,750] | 1,747,001 [1,722,126..1,777,041] | 500 [417..542] |
| append PI | 91,583 [89,583..96,833] | 1,244,375 [1,226,917..1,263,332] | 442,958 [435,167..453,043] | 3,021,544 [2,997,958..3,044,583] | 1,957,833 [1,943,542..1,976,166] | 834 [790..1,000] |
| remove first | 106,457 [102,458..117,332] | 15,375 [14,958..17,001] | 38,333 [38,251..38,543] | 250,376 [247,458..255,500] | 119,458 [117,791..121,042] | 501 [458..585] |
| remove middle | 106,999 [102,625..108,666] | 15,500 [15,167..17,458] | 38,375 [38,292..43,208] | 251,208 [248,208..261,834] | 119,207 [116,126..123,917] | 499 [416..584] |
| remove last | 107,209 [103,249..113,334] | 16,334 [15,207..60,083] | 38,416 [38,334..42,957] | 248,499 [246,333..253,875] | 122,501 [119,250..126,917] | 542 [500..583] |
| remove deep | 715,082 [702,459..729,417] | 54,208 [47,083..55,334] | 1,376 [1,333..1,459] | 5,418 [5,167..5,542] | 250,166 [241,709..260,917] | 750 [749..833] |
| churn | 92,376 [90,542..101,541] | 1,782,084 [1,770,166..1,849,125] | 59,000 [58,623..59,208] | 357,917 [355,918..362,250] | 419,417 [414,624..433,541] | 707 [625..7,042] |

The focused append rows each completed 5,000 calls. Churn completed 5,000
append/remove units, or 10,000 calls, while retaining the original 97 live
nodes and growing each document from 97 to 1,097 permanent slots. Deep removal
completed 80 calls and invalidated 5,200 nodes. The complete matrix also shows
the expected directional setup cost for UTF-16 decoding and edit cost growth
with larger copied payloads; for example, its selected UTF-8 churn row moved
from 1,745,750 ns at 32 bytes to 6,855,958 ns at 1,024 bytes. These observations
only confirm that the harness exposes the intended work dimensions.

## Validation

`cargo test --all-targets --offline --locked` passed, including all 54 bounded
debug benchmark rows. The benchmark support tests cover accepted and rejected
case labels, every counter overflow/underflow result without partial updates,
append/remove/churn accounting and first/middle/last/deep target order. The
following also passed on the implementation commit:

```console
cargo fmt --check
cargo clippy --all-targets --offline --locked -- -D warnings
cargo test --test structural_benchmark --offline --locked
```

Raw artifacts:

| Artifact | SHA-256 |
| --- | --- |
| `/private/tmp/yaxml-structural-bench-release-7b217d4.txt` | `e8891e4b995af81c73f593a2aea083f49b7c2325a6c6aea1c857285649cc6a48` |
| `/private/tmp/yaxml-structural-bench-focused-7b217d4.txt` | `987da7fdad3e73b45c0eaf5e471b1bee5d10c8b5b6399eb1f82848e32ebfbc24` |
| `/private/tmp/yaxml-structural-bench-all-targets-7b217d4.txt` | `2ff42bffbda26424a47db65752a15c49e5facbc3d602a7291c3fa52c4683b90c` |

## Limits

No matched C structural-edit helper was available, so every row emits
`comparison_status=unmeasured` and only names the intended libxml2 lifecycle.
The host was not isolated, the focused run is small, allocation counts and peak
memory were not measured, and sub-microsecond destruction timings approach the
clock and allocator-cache noise floor. A later comparison must validate equal
logical output, account for libxml2 Text coalescing, match node construction and
unlink/free boundaries, and repeat on controlled hosts before drawing a
performance conclusion.
