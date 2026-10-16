# Matched owned structural benchmark validation

This report records the first matched structural-edit comparison at code commit
`22fe425d65b91dbddd7d50dd06a3f17ce85a15d3` on 2026-10-16. The correctness
gate passed: all 168 retained pairs matched exact final UTF-8 bytes, output
hashes, deterministic operation counts and the permitted Text-node topology
difference. The raw stderr file was empty.

The performance result does not support an across-the-board speed claim. Using
5% as a descriptive materiality threshold, yaxml had the higher median in 106
of 144 workload/phase rows, libxml2 had the higher median in 37, and one row was
within 5%. Several phases intentionally measure different observer work, so
these counts are a finding list rather than an aggregate score.

## Build and host

- Host: Apple arm64, Darwin 27.0.0, kernel build
  `RELEASE_ARM64_T6000`.
- Rust: `rustc 1.95.0 (59807616e 2026-04-14)`, LLVM 22.1.2, and
  `cargo 1.95.0`. The Cargo release and bench profiles use thin LTO and one
  codegen unit.
- C: Apple Clang 21.0.0 (`clang-2100.3.34.2`), target
  `arm64-apple-darwin27.0.0`.
- libxml2: compiled and runtime version 2.15.4 with tree and output support.
  The static build used `CMAKE_BUILD_TYPE=Release`, no shared library, and no
  iconv, ISO-8859-X, ICU, zlib, Python, docs or tests. XPath and output support
  were enabled. The previously verified upstream archive SHA-256 was
  `98087fd181d9070724f3fbc65c7377db03038eb92bd882374daff44940138821`.
- C helper flags: `-std=c11 -O3 -DNDEBUG -Wall -Wextra -Werror -pedantic`.
  The static archive was already built in Release mode.

The helper build was external to Cargo:

```console
cargo build --release --example structure_benchmark --offline --locked
cargo bench --bench structural_mutation --no-run --offline --locked \
  --message-format=json
cc -std=c11 -O3 -DNDEBUG -Wall -Wextra -Werror -pedantic \
  -I/private/tmp/yaxml-oracle-21504/build \
  -I/private/tmp/yaxml-oracle-21504/libxml2-2.15.4/include \
  tooling/libxml2_structure_bench.c \
  /private/tmp/yaxml-oracle-21504/build/libxml2.a \
  -lm -pthread -ldl \
  -o /private/tmp/yaxml-libxml2-structure-bench-22fe425
```

## Matrix and sampling

The base matrix covered all nine cases at width 16, depth 64, 32-byte payload
and UTF-8. Append and churn rows used 50 fresh documents with 1,000 edit units
each. Removal rows used 200 fresh documents; each removed all 16 available
targets because planned operations are `min(width, 1000)`.

`append_text`, `remove_middle` and `churn` also covered these axes:

- width 256 at depth 64;
- depth 1 at width 16;
- 1,024-byte payloads;
- UTF-16LE and UTF-16BE input.

The payload append/churn workloads used 25 documents and 500 units. The wide
removal workload used 50 documents. Other axis rows retained the base iteration
and operation counts. Every row ran 16 live-traversal passes and four counting
serializations per document.

Each workload had seven retained samples. Odd samples ran yaxml first and even
samples ran libxml2 first. A fixture/version warm-up ran before the retained
samples. Both implementations ran as prebuilt executables under the same
60-second deadline and 1 MiB output ceiling. Process startup and fixture export
were outside all reported inner clocks.

The exact matrix command sequence is in
`/private/tmp/yaxml-structure-matched-run-22fe425.sh`. The parser and summarizer
are `/private/tmp/analyze-yaxml-structure-22fe425.py` and
`/private/tmp/table-yaxml-structure-22fe425.py`.

## Phase boundaries and limits

Both sides parse a fresh document, select the required parent and preselect
removal targets in setup. C counts removed subtrees in setup. Its exact initial
whole-document count runs outside every clock because yaxml obtains its arena
length in constant time. The edit clock includes fresh node construction and
addition, or unlink plus `xmlFreeNode`. Pinned libxml2 implements the latter's
node-list freeing iteratively. C uses scalar edit counters. The Rust clock also
includes allocation and pushes for the live/stale handle vectors.

The C helper places C11 signal fences around every timed traversal call and
folds each pass into a non-cancelling checksum. Apple Clang 21 LLVM IR contained
`fence syncscope("singlethread") seq_cst` on both sides of each retained
`visit_tree` call. The earlier helper also retained these calls on this
compiler; the fence makes that property explicit.

Common traversal work observes node kind, qualified-name and namespace lengths,
direct payload length and first byte, and attributes. Rust additionally reads
source-provenance ranges. libxml2 does not retain comparable provenance.

Serialization also differs below the byte contract. libxml2's `xmlSave`
buffers internally and commonly makes one counting-sink call. The Rust writer
calls its counting sink directly. Validation work is more extensive in Rust:
it checks the live/stale handle sets, serializes, reparses and validates the
edited model. The C validation clock serializes to an `xmlBuffer`, compares
exact bytes and computes FNV. Treat serialization and validation ratios as
measured lifecycle costs, not isolated serializer or parser comparisons.

yaxml enforces the default 1 MiB value, 64 MiB cumulative owned-byte and
1,000,000 created-node limits. It retains removed tombstone slots to keep
handles stable. libxml2 has no matching quotas and frees unlinked nodes
immediately.

## Exact-match result

The driver validated each row against its requested configuration and derived
counts before comparing the pair. It rejected missing, duplicate, nonnumeric,
nonfinite and mismatched schema, configuration or accounting fields. It also
required exact output byte totals and hashes. Unknown additional fields are
accepted for forward-compatible row extensions. All 24 workloads times seven
samples passed.

libxml2 coalesced the 1,000 adjacent appended Text nodes into one survivor per
document. yaxml retained 1,000 distinct live Text handles. The base Text row
therefore traversed 1,097 live yaxml records and 98 live libxml2 nodes, while
serializing identical bytes. This topology difference applies to every Text
row and makes their traversal and serialization ratios non-equivalent. It also
makes the C edit clock pay repeated Text concatenation; those edit ratios do not
establish a general structural-insertion advantage. Empty PI payloads remain
excluded because libxml2 emits `<?bench ?>` while yaxml emits `<?bench?>`. All
other empty leaf cases passed an additional smoke check.

## Edit medians and spread

The ratio is `yaxml median / libxml2 median`; values below 1 favor yaxml. The
range contains all seven retained samples.

| workload | yaxml edit median [min, max] ms | libxml2 edit median [min, max] ms | ratio |
|---|---:|---:|---:|
| base_append_text | 10.171 [10.148, 10.525] | 31.838 [31.701, 32.553] | 0.32x |
| base_append_cdata | 11.256 [11.221, 11.309] | 1.235 [1.223, 1.240] | 9.11x |
| base_append_comment | 11.370 [11.348, 11.393] | 1.351 [1.338, 1.355] | 8.42x |
| base_append_pi | 12.041 [12.016, 12.116] | 2.078 [2.064, 2.089] | 5.79x |
| base_churn | 17.531 [17.479, 17.793] | 2.555 [2.540, 2.670] | 6.86x |
| base_remove_first | 0.546 [0.540, 0.563] | 0.142 [0.133, 0.155] | 3.85x |
| base_remove_middle | 0.558 [0.553, 0.567] | 0.142 [0.135, 0.155] | 3.93x |
| base_remove_last | 0.546 [0.542, 0.559] | 0.138 [0.134, 0.154] | 3.96x |
| base_remove_deep | 1.828 [1.799, 1.835] | 3.099 [3.079, 3.119] | 0.59x |
| wide_append_text | 10.144 [10.109, 10.308] | 31.784 [31.634, 32.064] | 0.32x |
| shallow_append_text | 2.657 [2.643, 2.668] | 31.790 [31.706, 32.106] | 0.08x |
| payload_append_text | 15.310 [15.244, 15.607] | 118.171 [117.491, 119.046] | 0.13x |
| utf16le_append_text | 10.204 [10.174, 10.281] | 31.864 [31.721, 31.982] | 0.32x |
| utf16be_append_text | 10.194 [10.133, 10.206] | 31.750 [31.693, 31.828] | 0.32x |
| wide_churn | 17.524 [17.481, 17.597] | 2.541 [2.518, 2.567] | 6.90x |
| shallow_churn | 2.949 [2.919, 3.006] | 2.605 [2.551, 2.615] | 1.13x |
| payload_churn | 17.126 [17.094, 17.214] | 1.260 [1.246, 1.274] | 13.59x |
| utf16le_churn | 17.564 [17.467, 17.598] | 2.576 [2.551, 2.592] | 6.82x |
| utf16be_churn | 17.490 [17.474, 17.616] | 2.551 [2.545, 2.583] | 6.86x |
| wide_remove_middle | 2.038 [2.006, 2.096] | 0.533 [0.524, 0.551] | 3.82x |
| shallow_remove_middle | 0.090 [0.087, 0.094] | 0.162 [0.131, 0.167] | 0.56x |
| payload_remove_middle | 0.557 [0.552, 0.569] | 0.144 [0.138, 0.155] | 3.87x |
| utf16le_remove_middle | 0.562 [0.558, 0.574] | 0.143 [0.131, 0.159] | 3.93x |
| utf16be_remove_middle | 0.554 [0.552, 0.571] | 0.146 [0.129, 0.148] | 3.79x |

Across all phase distributions, the median relative median absolute deviation
was 0.49% and the 95th percentile was 4.55%. The maximum was 16% for libxml2's
25-microsecond shallow-removal traversal total. `clock_gettime` results from the
C helper were quantized to microseconds on this host. Ratios involving sub-ms C
totals, especially traversal, drop and some removal edit rows, need larger
batches before they can support fine-grained conclusions.

At the 5% descriptive threshold, the phase counts were:

| phase | yaxml slower | libxml2 slower | within 5% |
|---|---:|---:|---:|
| setup | 24 | 0 | 0 |
| edit | 16 | 8 | 0 |
| traversal | 20 | 3 | 1 |
| serialization | 22 | 2 | 0 |
| validation | 24 | 0 | 0 |
| drop | 0 | 24 | 0 |

The full 144-row median, min, max, MAD and ratio table is
`/private/tmp/yaxml-structure-matched-release-22fe425-summary.tsv`. Setup
includes yaxml quota/provenance construction, validation differs as described,
and Text traversal/serialization has different live-node work. Drop times are
small and compare tombstone-arena destruction with libxml2's iterative node-list
freeing. These phase counts must stay attached to those qualifications.

The first, middle and last base removals delete leaves below a depth-64 parent,
so yaxml updates subtree counts through the ancestor chain. Their 3.85x--3.96x
edit ratios identify that expected depth-dependent maintenance as an
optimization target. Deep removal was the only base removal where yaxml had the
lower median. That case detaches depth-64 subtrees directly from the root:
yaxml marks the subtree slots stale and updates only the short root/document
ancestor chain, while libxml2 iteratively frees the detached node list. This
fixture does not establish a general complexity advantage. Shallow middle
removal also favored yaxml, but both totals were below 0.2 ms per aggregate
sample.

## Verification and artifacts

The exact code commit passed:

```console
cargo fmt --all -- --check
cargo clippy --all-targets --offline --locked -- -D warnings
cargo test --all-targets --offline --locked
```

Strict optimized C compilation passed. The all-nine UTF-8 smoke matrix, six
representative UTF-16 rows and eight allowed empty-payload rows passed. A
truncated request and an exact-byte mismatch each exited 65. Empty PI was
rejected before either helper ran.

Artifact SHA-256 values:

| artifact | SHA-256 |
|---|---|
| raw 24-workload log | `5bc890e0f44a7f7652f72c3530d027406d7ddf7d33f634b59af6c5fda4bea04f` |
| raw stderr (empty) | `e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855` |
| full summary TSV | `96f4a7ecc54731b5ebae0cdb9b92026dda7afb2364d7eff9bc161e8725562f52` |
| raw analysis output | `6f3bc9ad41981f149dd5a215fbe100bcb9982f56c14ae741aebff5a47ef88e93` |
| matrix runner | `55b6969e873b456380369ca6a68c645c5ec5c3d6285958e2b24b81627f402862` |
| summary parser | `f8bbb659f4cd05550cebeac65577600194aa9252f6166d74a08be72d40ea4627` |
| edit-table formatter | `45ca41d7989d17acd62a7d0065f04302c5bc58d5c007c8acc9cdaa6b9cf64114` |
| exact all-target test log | `7f488e7b18192f92d66d078c3ab10dfe7c36340d6389c06e08bc87d1177f706e` |
| failure-gate log | `cd8d846e5bd41f153d68eda642ceea7bca9430c977ca7b88de050dabb4bc09b2` |
| optimized C helper | `428be57a414b41d02d4890322b9787e33467da21603ede6a21c8780c6958aa55` |
| Rust driver | `0c62f891bec0011d9f4f81c6f71ec2518516b70a59a6ec82553f57155b9d6cf7` |
| Rust benchmark executable | `aa0b18c267ff743bb3d792ae98d0861f10658659c317fc6ce0790f0072e080c8` |
| pinned static libxml2 archive | `b48612c66e7d4d7fae1ceb7db9a0d3e288085bc91b560f037862607eccac1ab0` |
| C helper source | `e720dc3789cd8c47cc379a9721033c8f2bdc76fbabd8ec97434d197090344883` |
| Rust benchmark source | `cd40d1a39483ef981f049cd0223f7d0ceb5d2168f31b8450e2091abe7c1a031b` |
| Rust comparison parser source | `de5b43485b6854c4d9fbe2722ac90ba28cb3a4bbeff28d1e348d80635fa27ce6` |

The raw artifacts remain under `/private/tmp` with the
`yaxml-structure-matched-*22fe425*` prefix. This is a single-host directional
run. It does not isolate allocator behavior, CPU frequency changes or internal
algorithm cost, and it does not support a blanket performance claim.
