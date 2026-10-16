# Owned attribute membership representation experiment

This report records an experiment based on `abdd70951127ae936b7238eb35670a25a4954ab6`.
It does not freeze a representation or public API. Neither prototype should be
merged into the production branch. The side-index prototype preserves the
compact record layout and behaves correctly, but it fails the default sparse
traversal performance gate.

## Decision

Reject both tested representations for the next production milestone.

- Embedding `Baseline(Range<usize>) | Edited(Vec<StoredAttribute>)` in every
  element makes `Content` require drop glue. That changes `OwnedRecord` from 144
  to 152 bytes and makes destruction walk every record. Attribute-free document
  destruction regressed from a median 2.46 us to 308 us for a batch of 32
  documents (+12,431%). Sparse traversal also regressed 28.5%.
- Separate `Content::ElementBaseline` and `Content::ElementEdited(buffer_index)`
  variants with a document-owned `Vec<Vec<StoredAttribute>>` restore 64-byte
  `Content`, 144-byte `OwnedRecord`, and `needs_drop == false` for both. Parsed
  documents allocate no edited buffer. Parse and serialization stay within 1%
  of the unchanged baseline. The exact public traversal observer is still
  43.8% slower for zero-attribute elements, 30.3% slower when most elements are
  attribute-free, and 11.6% slower with one attribute per element. These rows
  have relative median absolute deviation at or below 0.2%.

The two-variant side-index layout remains useful evidence, not an accepted
implementation. Investigate optimized compiler output for the combined public
accessor loop as a separate bounded task. Keep the next representation
experiment limited to preserving the current parsed `Content::Element` fast
path, 144-byte dropless records, borrowed exact-size attribute views, and no
default-path allocation. Do not start the construction API or membership
counter design until a candidate clears the existing public parse, traversal,
serialization, and destruction matrix.

## Prototypes and invariants

The experiment branch contains these checkpoints:

| Commit | Purpose |
| --- | --- |
| `52ce7fa` | Embedded lazy enum; rejected because every record acquires drop glue |
| `27e7ba1` | Two `Content` variants plus a document-owned edited-buffer arena |
| `75f1b7a` | Ignored private benchmark helper for actual edited elements |

The side-index prototype keeps parsed attributes in the existing global
contiguous vector. A private test-only migration helper copies one element's
attribute metadata into a side buffer, reserves the outer arena, pushes the
buffer, and only then publishes `ElementEdited`. The normal parser produces only
baseline elements and an empty side arena. Existing value edits update whichever
storage currently owns the element's metadata; they do not trigger membership
migration.

Public `attributes()` still returns a borrowed exact-size iterator over a slice.
No public attribute ID or membership API was added. The `Attribute` lifetime
remains tied to the document borrow. Edited buffers retain copies of parsed
metadata while the original global entries remain allocated, so changed
elements consume duplicate metadata until document destruction. Empty edited
buffers need no inner allocation, but every migrated element occupies an outer
arena entry. A future real membership operation would expose both the inner
buffer reservation and outer arena reservation as explicit fallible boundaries.

Tests cover both variants, exact-size and fused iteration, value editing,
serialization, empty membership, idempotent migration, and allocation failure at
both reservation boundaries. Each injected failure leaves the element in its
baseline variant, leaves the side arena empty, and preserves the pool, values,
and counters; a later retry succeeds. Layout tests assert that `Content` and
`OwnedRecord` need no drop glue. On this 64-bit target the measured sizes are:

| Type | Baseline | Embedded `Vec` | Side index |
| --- | ---: | ---: | ---: |
| `Content` | 64 B | 72 B | 64 B |
| `OwnedRecord` | 144 B | 152 B | 144 B |
| `StoredAttribute` | 64 B | 64 B | 64 B |

The mixed immutable-baseline/edited-buffer view is manageable at this size: the
parser and existing value edit keep their direct paths, while only membership
changes copy metadata. An always-linked representation would impose dependent
loads on every parsed attribute and should remain rejected by the earlier
synthetic evidence. Adding a third linked view beside contiguous baseline and
edited buffers would add dispatch, iterator, serializer, quota, and failure
cases without evidence that it solves the observed compiler regression. Do not
add that complexity in the next experiment.

## Frozen public-path measurement

The external runner is identical for baseline and candidate. Fixtures are built
before clocks. Parse timing excludes document destruction. Traversal observes
node kind, qualified/local/namespace name lengths, attribute values, and exact
node/attribute counts. A representative ordinary attribute value edit performs
10,000 public edits. Serialization uses the same counting `Write` sink.
Validation, reparsing, and setup have separate clocks. Destruction uses a batch
of independently parsed documents. All output hashes, counts, iterations, and
checksums must match before the analyzer accepts a row.

Both binaries use the release profile `codegen-units=1`, thin LTO, and
`panic=abort`, with the locked `memchr 2.8.3`. Each matrix retains 31 samples
after one warm sample. Sample parity reverses fixture order and alternates binary
order. The matrix covers 0/1/2 attributes, mostly attribute-free elements,
8/32/128 dense attributes, namespaces, depth 2,048, and UTF-16 input.

Selected side-index medians relative to unchanged `abdd709`:

| Fixture | Parse | Traverse | Value edit | Serialize | Drop |
| --- | ---: | ---: | ---: | ---: | ---: |
| sparse0 | +0.1% | **+43.8%** | n/a | -0.3% | +5.1% (2.46 to 2.58 us, noisy) |
| sparse1 | -0.1% | **+11.6%** | **+5.7%** | +0.1% | +0.7% |
| sparse2 | -0.2% | +3.0% | +4.0% | +0.1% | +0.6% |
| mostly empty | -0.1% | **+30.3%** | +2.1% | -0.1% | +0.9% |
| dense8 | -0.1% | -2.1% | +1.9% | -0.1% | -1.2% |
| dense32 | +0.1% | -4.6% | -1.3% | -0.4% | +2.1% |
| dense128 | +0.1% | -4.3% | -0.6% | -0.3% | +19.2% (1.08 to 1.29 us, noisy) |
| namespaces | -0.1% | -2.2% | -3.4% | -0.2% | +3.8% |
| deep | +0.1% | +3.1% | +3.9% | +1.0% | 0.0% |
| UTF-16 | -0.8% | -2.1% | +2.0% | -0.4% | +1.3% |

Drop ratios on sub-microsecond-to-microsecond batches have broad relative spread
and are not evidence of a per-record destruction regression. The side-index
default document has an empty outer arena and dropless records; this removes the
embedded enum's O(records) destruction work.

## Combined-loop diagnostic

A second external runner times node iteration, `kind()`, `name()`, attribute
length, attribute visiting, and the exact combined observer independently over
the same parsed documents. It retains 31 counterbalanced samples per row.

For `sparse0`, node iteration is +0.0%, `kind()` -0.2%, `name()` +0.2%,
`attributes().len()` +3.0%, and standalone attribute visiting +5.3%. The exact
combined loop is +43.5%. `mostly_empty` shows +2.9% for attribute length, -2.3%
for standalone attribute visiting, and +33.9% for the combined loop. This points
to optimized combined-loop code shape rather than record size, allocation, or
any one accessor in isolation.

Two deliberately separate code-shape experiments did not fix it. Restoring the
old `Element` name/discriminant position and appending the edited variant left
the sparse0 combined loop at +44.7%. Moving edited arms behind cold, non-inlined
helpers made it +99.1%. Both experiments were reverted. These measurements do
not prove a compiler defect; the next task should inspect generated optimized
IR/assembly before changing another representation.

## Actual edited-element measurement

The ignored unit-test helper constructs the same real `OwnedDocument`, migrates
either one element or every element outside the phase clocks, and uses the
public traversal, value-edit, and writer paths. It compares baseline and edited
documents in alternating order, validates exact serialized bytes, and records
31 retained samples for seven sparse/dense fixtures. If attributes exist, the
one-element case selects an attribute-bearing element and reports the actual
10,000 edit calls. The first raw attempt selected an empty mostly-empty element
while reporting edit calls; that file is retained with
`superseded-target-accounting-bug` in its name and is excluded from results.

With one migrated element, traversal and serialization remain within 1.2% and
value edits within 3.0%. Drop ratios range from +6% to +30%, but the absolute
extra destruction for eight documents is only about 0.2 to 1.0 us and those
rows have roughly 9% to 24% relative MAD.

With every element migrated, traversal, serialization, and value editing remain
within 4.2%. Destruction now releases every side buffer. Reported per-document
medians include all buffer and outer-arena destruction: sparse0 2.03 us,
sparse1 38.51 us, sparse2 44.68 us, mostly-empty 5.47 us, dense8 15.04 us,
dense32 171.62 us, and dense128 14.28 us. This is per-document destruction cost,
not a record-size effect. It reflects buffer allocation count/size and allocator
behavior; the dense32/dense128 ordering shows that attribute count alone does
not predict it.

Membership setup is separate from parse and other phases. One migration takes a
median 0.42 to 1.00 us across these fixtures. Migrating all elements takes 39 us
for sparse0, 118 us for sparse1, 148 us for sparse2, 59 us for mostly-empty,
81 us for dense8, 237 us for dense32, and 434 us for dense128. Those times
include copying parsed metadata and both reservation boundaries.

## Reproduction

Host and compiler:

```text
MacBookPro18,2; Apple M1 Max; 10 cores (8 performance, 2 efficiency); 64 GB
Darwin 27.0.0 arm64
rustc 1.95.0 (59807616e 2026-04-14), LLVM 22.1.2
cargo 1.95.0 (f2d3ce0bd 2026-03-21)
```

The external projects are `/private/tmp/yaxml-attribute-ab-runner` and
`/private/tmp/yaxml-attribute-diagnostic`. Their checked-in path dependency is
switched between the detached baseline worktree and the experiment worktree
before each release build. Representative commands:

```sh
cargo build --release --offline --locked
python3 /private/tmp/yaxml-attribute-ab-runner/run_ab_side.py 31 \
  /private/tmp/yaxml-attribute-side-index-ab-raw-31.txt
python3 /private/tmp/yaxml-attribute-ab-runner/analyze_ab_side.py \
  /private/tmp/yaxml-attribute-side-index-ab-raw-31.txt

cd /private/tmp/yaxml-attribute-diagnostic
sh run.sh
ruby analyze.rb /private/tmp/yaxml-attribute-diagnostic-raw-31.txt
sh run_edited.sh
ruby analyze_edited.rb /private/tmp/yaxml-attribute-edited-raw-31.txt

cd /private/tmp/yaxml-attribute-membership
cargo fmt --check
cargo clippy --offline --locked --all-targets -- -D warnings
cargo test --offline --locked --all-targets
```

The final checks pass: 84 unit tests passed with the benchmark helper ignored,
the complete all-target suite passed, strict all-target Clippy passed, and
formatting passed.

Primary evidence hashes:

| Artifact | SHA-256 |
| --- | --- |
| Public runner source | `ce25194ef31c2a65853acc5daf901c395976bb6d66316569197fe1503b329859` |
| Public runner lockfile | `2ea5ded23c18de19f8f3fd5084b0f3be821b10c420923e02c7d13d42bedbd18f` |
| Baseline public binary | `e93043304d148e048afd7530d702029253c2bcc1556f1efe0bcb0e2d5df39784` |
| Embedded-enum public binary | `08f7ab85c3af4abcf35fbddbf3b29630030ff5800eadf682f702cf967d2d2cf8` |
| Side-index public binary | `c9387785af72dbe9944330e84e515b23fb07342a1a87ca3c21701358ca8c6bc0` |
| Embedded raw / summary | `38673af4256a979055f81829326e1c9f686e663af2b0bfbed1e509d944453874` / `7113955e57091d94b48e5de68e6ac611a3c23bbc62bf5745647b9e54b00d3459` |
| Side-index raw / summary | `98de0885c46429f83c5f877238cf1fbf234d9b2c83aff4832e4717f7d6e09381` / `7c2223f804c79a1b67a955b966cae0a0ca3e35cc625d3789fffc8066067745dd` |
| Diagnostic source | `7916e02b8ebfa2b6021e0aad602991058f313726a09bd32e1095605a52b3b1ff` |
| Diagnostic raw / summary | `090532f5f737ff65bd728fac3755854b4a2685179eac364c9be283b510d1d3c6` / `60da564c6f51454426c96d3257de95ef1ba1a69218336f79399239c8369e3029` |
| Edited helper driver / analyzer | `3d1d18f77909fa98007b0647e3a31c96b458ba3278f9b3e75812f4f9f388e43a` / `21a3522b6a5c697ca30e2e19ece3cdc44a4e4ce49907b45679aa60815f9b2a91` |
| Edited raw / summary | `8e468023764842d12adfc539e50fd952701c603ea6b16be35d83abd5704e381f` / `830b185bde0b0e167961472b06177a47b5c8f04891fb188cbccbfc059c308a47` |

The full shape/cold diagnostic artifacts and their hashes remain under
`/private/tmp/yaxml-attribute-diagnostic-*`. No result here supports a general
performance claim against libxml2 or any other parser.
