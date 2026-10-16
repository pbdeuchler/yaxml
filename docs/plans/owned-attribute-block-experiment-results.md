# Contiguous owned-attribute block experiment results

This report records the bounded experiment planned in
[owned-attribute-block-experiment.md](owned-attribute-block-experiment.md). It
compares unchanged `9e4b3a6ce23014754be997688ff6af29a308e7ef` with the
minimum-four prototype through `5e70a293b512efa2405403d85dbd230e1953f288`
and the minimum-one follow-on at
`2936a877f8dda0dd5d88c47232c26ac67122520b`. Both prototypes must remain
off the production branch.

## Decision

Reject both tested initial-capacity policies. Keep the representation as useful
evidence, but do not freeze its metadata, limits, errors, or construction API.

The minimum-four parsed path clears the experiment's primary gate. Across 31
fresh retained samples for ten fixtures, no parse, traversal, existing-value
edit, serialization, or destruction median regresses by 5%. `Content` stays 64
bytes, `OwnedRecord` stays 144 bytes and dropless, and optimized public
attribute iteration still loads the element's existing `Range<usize>` directly.
Parsed documents allocate no descriptor buffer.

The minimum-four edited-state screen exposes a stable sparse cost. Migrating
every element in the mostly-empty fixture makes the identical public observer
6.6% slower (4.867 ms to 5.188 ms); relative MAD is 0.1% and 0.2%. That fixture
has 4,097 elements and only 2,048 live attributes. Giving every empty edited
element a four-slot block expands the global storage to 19,460 physical slots,
of which 17,412 are obsolete or spare. Live attribute blocks become interleaved
with empty reserved blocks, which is consistent with the measured locality
loss. The benchmark does not establish a microarchitectural cause. Reducing the
initial capacity to one cuts the same fixture to 7,937 physical slots, but its
traversal remains 6.4% slower (5.011 ms to 5.331 ms, with 0.8% and 0.3% relative
MAD). Minimum one therefore fails the same edited-state gate.

A future candidate would need a descriptor state that represents an edited
empty range without a physical slot, or it would need to compact/reclaim empty
blocks. Either choice changes descriptor uniqueness and failure/reclamation
rules, so it needs a separate plan and should first target the same all-element
mostly-empty observer. Do not combine that work with provenance compression,
tagged ranges, or public attribute mutation.

## Prototype

`Content::Element` and every public attribute accessor are unchanged. The
prototype adds an append-only, start-sorted `Vec<AttributeBlock>` to
`OwnedDocument`; each 24-byte descriptor stores block start, owner `NodeId`, and
capacity. The owner is part of lookup, so another parsed empty `0..0` range
cannot claim a block beginning at zero.

A private test helper handles membership storage only:

1. Validate the handle, element kind, arithmetic, total physical-slot ceiling,
   and cumulative metadata-copy ceiling.
2. Find the current owner-qualified block by range start.
3. Reuse spare capacity, or choose the next power of two for the first block and
   double on later growth. The rejected first checkpoint additionally imposed a
   minimum of four slots.
4. Fallibly reserve both the global attribute vector and descriptor vector.
5. Copy live metadata, fill spare physical slots with private vacant values,
   append the descriptor, and only then publish the element's new range.
6. Append into spare capacity, or shift and vacate within the current block for
   removal.

Old blocks remain in the global vector. For one edited element, the sum of prior
geometric blocks is bounded by a constant multiple of the largest block, plus
the parsed baseline copy. Across elements, total retained storage is the parsed
baseline plus the sum of each edited element's allocated blocks. Descriptors
and physical slots are not reclaimed in this experiment.

Existing value edits continue to replace only the value range inside the live
`StoredAttribute`; they never trigger membership migration. The public iterator
still borrows the document slice and remains exact-size. No public API, new
dependency, unsafe code, quota contract, or attribute identifier was added.

## Correctness and failure evidence

Six focused tests cover:

- parsed nonempty and empty ranges with no descriptor allocation;
- owner disambiguation when the first block starts at zero;
- four spare-capacity appends followed by geometric growth;
- first, middle, last, and sole removal with stable attribute order;
- exact-size borrowed iteration, value replacement, and serialization through a
  migrated range;
- foreign and wrong-kind handles, missing positions, arithmetic overflow, total
  slot and copy ceilings, both reservation failpoints, retry, and unchanged
  values/output after failure.

All fallible reservations happen before vector lengths, descriptors, ranges,
values, or counters change. A successful attribute-vector reservation followed
by a descriptor-reservation failure may retain allocation capacity, matching
`Vec::try_reserve_exact`, but logical state remains unchanged. Lowering either
cumulative ceiling below already recorded work rejects even when the current
block has spare capacity.

The final minimum-one checkpoint passed
`cargo test --offline --locked --all-targets`: 87 library tests passed, the
private benchmark was ignored by default, and every integration, bench, and
example target passed. Strict all-target Clippy and formatting also pass. The
ignored release benchmark passed its exact output, checksum, and accounting
gates.

Three deliberate fault gates verify crucial assertions. Removing descriptor owner
qualification makes the shared parsed `0..0` isolation test fail with the first
element's block returned for the second. Replacing geometric doubling with
linear growth makes the append/growth accounting test fail at 30 physical slots
instead of the required 37. Skipping the descriptor-vector reservation check
makes the allocation-failure test observe a published block instead of an
atomic error. The source was restored after each mutation and before the final
checks; all three failure logs are retained with the other artifacts.

## Layout and optimized read path

Measured 64-bit layouts:

| Type | `9e4b3a6` | Prototype |
| --- | ---: | ---: |
| `Content` | 64 B | 64 B |
| `OwnedRecord` | 144 B | 144 B |
| `StoredAttribute` | 64 B | 64 B |
| `OwnedDocument` | 160 B | 192 B |
| `AttributeBlock` | n/a | 24 B |

The document grows by one 24-byte descriptor `Vec` and one private 8-byte
experimental copy counter. The counter supports only the private cumulative
failure model and is not a proposed public metric.

The exact external combined observer was compiled with the same release profile
to LLVM IR and AArch64 assembly for both revisions. In the combined-loop IR,
the attribute range and `StoredAttribute` loop have the same control flow and
SSA operations after symbol hashes are ignored. The only structural differences
inside the extracted region are `OwnedDocument` field offsets moved by the new
document fields, including pool loads at offsets 104/112 becoming 128/136.
There is no descriptor lookup, new element variant, or membership branch in the
public attribute loop. Full IR, assembly, and extracted regions are retained in
the evidence directory. This establishes direct range access; it does not imply
identical instruction addresses, stack layout, or whole-program code placement.

## Parsed-path A/B

The external runner is byte-for-byte identical between builds. It measures
public parse, the combined node/name/attribute observer, 10,000 existing-value
edits where an attribute exists, serialization into the same counting writer,
and destruction of a separately prepared document batch. Setup, validation,
reparse, and drop setup are outside those clocks. Every accepted pair has equal
fixture metadata, node and attribute counts, iterations, traversal checksum,
serialized byte count, and output hash.

Both binaries use the locked `memchr 2.8.3` and release profile with thin LTO,
one codegen unit, and aborting panics. Fixture and binary order alternate. The
matrix retains 31 samples after one warm sample for sparse 0/1/2, mostly-empty,
dense 8/32/128, namespaces, depth 2,048, and UTF-16 input.

Selected fresh minimum-four medians relative to `9e4b3a6`:

| Fixture | Parse | Traverse | Value edit | Serialize | Drop |
| --- | ---: | ---: | ---: | ---: | ---: |
| sparse0 | +0.2% | +0.1% | n/a | +0.2% | 0.0% |
| sparse1 | -0.2% | +0.2% | -3.1% | +0.4% | +0.7% |
| sparse2 | +0.0% | -0.1% | +0.1% | +0.3% | -3.5% |
| mostly empty | +0.1% | -0.1% | -0.1% | +0.1% | 0.0% |
| dense8 | -0.1% | -0.2% | +0.0% | +0.0% | -1.8% |
| dense32 | +0.1% | -0.3% | +1.1% | -0.0% | -2.1% |
| dense128 | -0.0% | -0.0% | +0.4% | -0.0% | -22.8%* |
| namespaces | -0.2% | -0.1% | -0.1% | -0.4% | +0.7% |
| deep | -0.1% | -0.3% | +0.1% | +0.6% | -1.8% |
| UTF-16 | +0.1% | -0.0% | +0.0% | +0.1% | +1.3% |

`*` The dense128 drop medians are 1.458 and 1.125 microseconds for the whole
batch, with 31.4% and 11.1% relative MAD. These tiny noisy measurements are not
evidence of a destruction improvement. No parsed phase has a material median
slowdown under the experiment's 5% threshold.

The minimum-one commit was rebuilt and rerun rather than inferred from its
test-only policy change because the resulting executable hash changed. All
non-drop phases again remain below the 5% threshold: the largest positive
medians are +0.2% parse, +0.2% traversal, +0.7% existing-value edit, and +0.8%
serialization. Namespaced destruction reports +5.8%, from 6.459 to 6.833
microseconds for a batch of 30 documents. Its relative MADs are 3.9% and 6.1%
and its ranges overlap broadly, so this single tiny drop row is not stable
evidence of a parsed-path regression. The edited-state failure below rejects
minimum one independently.

## Minimum-four edited-state A/B

The ignored private driver parses a fresh document and migrates either one
selected element or every element before starting traversal, value-edit, or
serialization clocks. It alternates baseline/edited order within each sample
and retains 31 samples after warmup. Migration setup and preparation of the
eight-document drop batch have separate clocks. Exact serialized bytes and the
combined observer checksum must match.

This is a representation screen, not a public append/remove throughput result.
Both edited-state modes use the same prototype executable and document layout;
the baseline mode leaves membership unmigrated. The separate parsed-path A/B
above compares the original and prototype implementations.
The migration helper copies the current live metadata without changing XML
semantics. It can model an element that was edited and later returned to the
same membership, including an empty retained block. In `migration=all`, the
helper artificially migrates every element: for `mostly_empty`, 3,841 elements
have no attributes and 256 have eight. The exact empty-block state is also
reachable if each empty element receives an attribute and later removes its
last attribute, but this benchmark does not time those public operations or
claim that every parsed empty element normally allocates a block.

Non-drop medians with one migrated element stay within 1.7% for traversal, 0.3%
for value edits, and 0.4% for serialization. With all elements migrated,
value-edit and serialization medians stay within 0.5%; traversal results are:

| Fixture | Baseline median | Edited median | Delta | Relative MADs | Physical/live slots |
| --- | ---: | ---: | ---: | ---: | ---: |
| sparse0 | 4.368 ms | 4.368 ms | +0.0% | 0.1% / 0.1% | 16,388 / 0 |
| sparse1 | 5.285 ms | 5.285 ms | -0.0% | 0.1% / 0.1% | 20,484 / 4,096 |
| sparse2 | 5.366 ms | 5.393 ms | +0.5% | 0.1% / 0.4% | 24,580 / 8,192 |
| mostly empty | 4.867 ms | 5.188 ms | **+6.6%** | 0.1% / 0.2% | 19,460 / 2,048 |
| dense8 | 5.706 ms | 5.823 ms | +2.1% | 0.1% / 0.3% | 16,388 / 8,192 |
| dense32 | 6.295 ms | 6.282 ms | -0.2% | 0.3% / 0.4% | 65,540 / 32,768 |
| dense128 | 5.978 ms | 5.961 ms | -0.3% | 0.3% / 0.2% | 131,076 / 65,536 |

Migrating all elements takes median 394 to 1,469 microseconds across these
fixtures and copies each live attribute once. That clock includes editor setup,
owner-qualified descriptor lookup, both reservations when growth is required,
metadata copies, vacant-slot initialization, and publication.

Edited destruction releases the additional global attribute allocation and
block descriptor vector. Its ratios range from +9% to +114%, but the absolute
median difference for a batch of eight documents is about 0.2 to 1.4
microseconds, and relative MAD reaches 14%. These rows establish the measured
lifecycle cost at this scale; they do not isolate allocator work or justify a
per-record claim.

## Minimum-one follow-on

Commit `2936a87` changes only the initial capacity rule and exact accounting
expectations. Empty and one-attribute elements receive one slot; two, four,
eight, and larger initial memberships still use their next power of two, and
later growth still doubles. The same tests, observer, counters, clocks, order,
fixtures, and 31-sample method are retained.

Minimum one removes most empty-block padding, but not the copied parsed
metadata or one retained slot per edited-empty element:

| Fixture | Migrated elements | Physical/live slots | Obsolete or spare | Descriptors | Copied attributes |
| --- | ---: | ---: | ---: | ---: | ---: |
| sparse0 | 4,097 | 4,097 / 0 | 4,097 | 4,097 | 0 |
| sparse1 | 4,097 | 8,193 / 4,096 | 4,097 | 4,097 | 4,096 |
| sparse2 | 4,097 | 16,385 / 8,192 | 8,193 | 4,097 | 8,192 |
| mostly empty | 4,097 | 7,937 / 2,048 | 5,889 | 4,097 | 2,048 |
| dense8 | 1,025 | 16,385 / 8,192 | 8,193 | 1,025 | 8,192 |
| dense32 | 1,025 | 65,537 / 32,768 | 32,769 | 1,025 | 32,768 |
| dense128 | 513 | 131,073 / 65,536 | 65,537 | 513 | 65,536 |

The minimum-one all-element traversal medians are:

| Fixture | Baseline median | Edited median | Delta | Relative MADs |
| --- | ---: | ---: | ---: | ---: |
| sparse0 | 4.499 ms | 4.498 ms | -0.0% | 0.1% / 0.8% |
| sparse1 | 5.437 ms | 5.439 ms | +0.0% | 0.8% / 0.3% |
| sparse2 | 5.524 ms | 5.436 ms | -1.6% | 0.8% / 1.4% |
| mostly empty | 5.011 ms | 5.331 ms | **+6.4%** | 0.8% / 0.3% |
| dense8 | 5.877 ms | 5.959 ms | +1.4% | 0.3% / 1.2% |
| dense32 | 6.459 ms | 6.462 ms | +0.0% | 0.7% / 0.4% |
| dense128 | 6.144 ms | 6.134 ms | -0.2% | 0.7% / 0.7% |

The regression reproduces despite a 59% reduction in mostly-empty physical
slots from minimum four. The experiment therefore cannot attribute the gap only
to four-slot padding. A plausible contributor is that copied eight-attribute
blocks remain separated by retained one-slot empty blocks, but hardware-counter
or cache evidence would be required to establish that cause.

One-element non-drop medians stay within 1.0% traversal, 0.3% value edit, and
0.2% serialization. All-element value-edit and serialization medians stay
within 2.1% and 0.4%. Drop ratios remain large on absolute differences below two
microseconds for the eight-document batch and do not change the decision.

## Reproduction

Host and toolchain:

```text
MacBookPro18,2; Apple M1 Max; 10 cores (8 performance, 2 efficiency); 64 GB
macOS 27.0.1; Darwin 27.0.0 arm64
rustc 1.95.0 (59807616e 2026-04-14), LLVM 22.1.2
cargo 1.95.0 (f2d3ce0bd 2026-03-21)
uv 0.11.12; CPython 3.14 managed by uv
```

Baseline and prototype binaries were built sequentially from the same external
runner in `/private/tmp/yaxml-attribute-blocks-runner`; only its path dependency
changed. Representative commands:

```sh
cd /private/tmp/yaxml-attribute-blocks-runner
cargo build --release --offline --locked
cp target/release/yaxml-attribute-ab-runner \
  /private/tmp/yaxml-attribute-blocks-runner-baseline
# Change only the yaxml path from the detached baseline to the prototype.
cargo build --release --offline --locked
cp target/release/yaxml-attribute-ab-runner \
  /private/tmp/yaxml-attribute-blocks-runner-prototype

UV_CACHE_DIR=/private/tmp/yaxml-uv-cache \
  uv run --no-project --python 3.14 python run_ab_side.py 31 \
  /private/tmp/yaxml-attribute-blocks-evidence-20261016/parsed-ab-31.raw
UV_CACHE_DIR=/private/tmp/yaxml-uv-cache \
  uv run --no-project --python 3.14 python analyze_ab_side.py \
  /private/tmp/yaxml-attribute-blocks-evidence-20261016/parsed-ab-31.raw

UV_CACHE_DIR=/private/tmp/yaxml-uv-cache \
  uv run --no-project --python 3.14 python \
  /private/tmp/yaxml-attribute-blocks-evidence-20261016/run_edited.py 31 \
  /private/tmp/yaxml-attribute-blocks-evidence-20261016/edited-ab-31.raw
UV_CACHE_DIR=/private/tmp/yaxml-uv-cache \
  uv run --no-project --python 3.14 python \
  /private/tmp/yaxml-attribute-blocks-evidence-20261016/analyze_edited.py \
  /private/tmp/yaxml-attribute-blocks-evidence-20261016/edited-ab-31.raw

# The minimum-one rerun changes only the prototype binary/raw destinations.
UV_CACHE_DIR=/private/tmp/yaxml-uv-cache \
  uv run --no-project --python 3.14 python \
  /private/tmp/yaxml-attribute-blocks-evidence-20261016/run_parsed_min1.py 31 \
  /private/tmp/yaxml-attribute-blocks-evidence-20261016/parsed-min1-ab-31.raw
UV_CACHE_DIR=/private/tmp/yaxml-uv-cache \
  uv run --no-project --python 3.14 python \
  /private/tmp/yaxml-attribute-blocks-evidence-20261016/run_edited.py 31 \
  /private/tmp/yaxml-attribute-blocks-evidence-20261016/edited-min1-ab-31.raw

cd /private/tmp/yaxml-attribute-blocks-ir
cargo rustc --release --offline --locked -- --emit=llvm-ir,asm
```

The first 31-sample matrices may have overlapped an independently running fuzz
verification. They are retained with `overlap-superseded` in their names and are
excluded from every result above. The displayed results come from complete
restarts after all other agents reported that heavy work had stopped.

Primary artifact hashes:

| Artifact | SHA-256 |
| --- | --- |
| Repository lockfile | `80e8083ac336bb23d35218fdb65b88e6520b0b477aac2d3b6688317ba9e3d766` |
| External runner source / lockfile | `ce25194ef31c2a65853acc5daf901c395976bb6d66316569197fe1503b329859` / `2ea5ded23c18de19f8f3fd5084b0f3be821b10c420923e02c7d13d42bedbd18f` |
| Baseline / prototype binary | `c8dd2a26947d4a8cee6fb1f4ce7402554906f390bd349f11348b9fbbdf960b6c` / `15cb4136ca1b037e2a24e76971ac8315d61576257920ef8154381005b1302e72` |
| Parsed raw / report | `42559e382159b8de696b54d483ae7b7b4c33523ded0d2cd8ca3773ac2fed28e9` / `b6a164217ac5e75aba1d0315953e47292d84d4de8ef01b357a634e6d6967cd2c` |
| Edited driver / analyzer | `fc3265682cfb5aeed36e2c182ec78b7501b4ed5c5f159be4b16c4156330b7ca6` / `dbdeb63b44648cc8d1f8f82609c37569810853d2b4d783a7025e263c951b75cb` |
| Edited raw / report | `a97c5f0870142e8ba93d27835bd9fd5ab966c586bb660ceffe4e055f066ae560` / `31545f161768cc8ab512ab371d4709050856f9835dc19933de882965338269cf` |
| Minimum-one prototype binary / parsed driver | `2e668178b63f37f3a909950821dd354adf06600abba78026c1855b0cd48d3201` / `5b6b900379b11115b8a770b63bd5039472419f9a8dd25a082dc6e4549a274519` |
| Minimum-one parsed raw / report | `5f53628cbf1918f2e0eb9d3850fb34f93c7f0125ea03523782748d0ddef008ba` / `bd5f5401c877de1e8691932da9830552910e1d6634ceb63463d9d73391518246` |
| Minimum-one edited raw / report | `fc8e2f4d2b9d4f2bdf8188bd7ef75932f8ee997cbc98539ed0ab0f2e4e484be6` / `5f1f722241740a745ddf75356c920e0da24cb43d2df47a45799014cb17adb017` |
| Owner / growth / descriptor-reservation fault logs | `b2ce94808c06984991b2567ba45c04296362b2892a710e8eb6a778702dac2abb` / `42888d93b7bc949743d158fc85578c57a83f624281d8bf41743c52d5fba6fe26` / `1f66105c2948ecd51ddae0c349fa2cf0bfd425eb86ace8de488759c42d88d8a0` |
| Evidence manifest | `14c71efe274450e1811dfe7f47bc0bdcb30fff3c2b03a674a307d73d5e4d79af` |

`SHA256SUMS` and complete raw, summarized, superseded, IR, and assembly artifacts
are under `/private/tmp/yaxml-attribute-blocks-evidence-20261016`. No result in
this report supports a performance claim against libxml2 or another parser.
