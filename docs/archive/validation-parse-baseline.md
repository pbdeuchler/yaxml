# Matched DOM and streaming parsing baseline

The first matched parsing baseline does **not** meet the performance target.
Across 65 workloads, yaxml had the lower median in 19, libxml2 in 42, and four
were within a descriptive 5% band. These counts describe this synthetic matrix,
not an aggregate score or a claim about the full libxml2 feature set.

The immutable measurement checkpoint is
`233638ab308d4377762c8c5ef779f509c9a5327e`, with executable inputs frozen at
`dfbd5969db786548addd96e123052beaa3ac23b3`, based on main `a25b8a6`.
Production Rust, Cargo inputs, existing benchmarks and fuzz harnesses are
unchanged; the `src` tree remains `34f9acb22ae65b99fcea2cc1cc52e9ee6e036b3f`.
This milestone supplies a comparison gate before parser ownership/layout work.

## Work and timing contract

The [plan](plans/matched-parse-baseline.md) specifies UTF-8, XML 1.0,
namespace-aware, DTD-free inputs. Both sides parse the exact same bytes with
fresh contexts. The five yaxml modes are borrowing DOM, owned DOM, slice reader,
literal-chunk push, and buffered pull. The counterparts are `xmlReadMemory`,
`xmlReaderForMemory`, genuine SAX2 push parsing and `xmlReaderForIO` callbacks.

The primary time is **parse plus destruction**. DOM observation has a separate
clock and is excluded from the primary result. Streaming includes observing
every logical value, construction, final EOF/consumption checks and compiler
anchors; it is not a tokenizer-only clock. Input loading, process startup,
fixture generation and version/profile preflight are outside all clocks.
Per-iteration timing and compiler-anchor overhead remain included.

Every iteration must produce the same six counters and complete transcript
digest. The driver compares each retained row with the paired implementation
and the discarded preflight, then publishes only complete pairs. A separate
295-byte literal transcript anchors `[2, 2, 2, 4, 3, 2]` counters and digest
`63bbd2c6aa2a2ec9`; expected values do not come from either parser result.
This is a noncryptographic logical consistency check, not canonical XML.

The reference owns DOM strings where the borrowing DOM can retain slices.
The memory reader copies input on the C side. Buffered frontends have different
internal buffering and fragmentation. C DOM observation obtains allocated
attribute values and walks parent links; Rust observation uses borrowed views
and an explicit traversal stack. Those observer costs are excluded from primary
DOM time. Parser resource policies differ: this run requires successful work
below both finite policies, without claiming equivalent rejection limits.

## Host and builds

- Apple M1 Max, 10 cores, 64 GB, MacBookPro18,2; Darwin 27.0.0 arm64.
- Rust 1.95.0, LLVM 22.1.2; release optimization level 3, thin LTO and one codegen
  unit. Verbose build commands, effective flags and compiler identities are
  retained. The only Cargo dependency remains `memchr` 2.8.3.
- Apple Clang 21.0.0; C helper flags `-std=c11 -O3 -DNDEBUG -Wall -Wextra
  -Werror -pedantic -Wconversion -Wshadow`.
- Fresh static libxml2 2.15.4 Release build from the official archive, verified
  against SHA-256
  `98087fd181d9070724f3fbc65c7377db03038eb92bd882374daff44940138821`.
  Iconv, ISO-8859-X, ICU, zlib, Python, docs, tests and programs were disabled;
  XPath and output enabled. Full CMake options and effective C flags are retained.
- The new static archive has SHA-256
  `a7dec2ded28b38e5025cc273427a3950707742b4b9dfc181a6b60d41b3e7cf32`.
  This is a build identity, not a portable upstream archive digest.

Other agent compilation, test and benchmark jobs stopped before sampling.
This does not establish CPU pinning, thermal/governor control or an otherwise
idle operating system. The host-query command emitted an activation-lock-status
timeout warning before builds; it returned the recorded hardware information.
No successful helper invocation emitted diagnostics.

## Matrix and results

Nine deterministic [corpus shapes](../../examples/support/parse_benchmark.rs) cover
tiny, flat, long text, 128 attributes per element, namespace scopes, normalization,
Unicode names/values, depth 128 and mixed lexical content. All five modes use
8,192-byte chunks where applicable. Push and buffered modes also use 1/7-byte
chunks for tiny, text, namespaces, normalized and mixed cases: **65 workloads**.
The Unicode fixture uses an ASCII namespace URI; the draft Unicode URI emitted
a reference namespace diagnostic and was excluded during corpus preflight.

Each workload has one retained calibration pair, then seven measured pairs in
alternating order, each helper in a fresh process. Each driver invocation also
discards its own preflight pair. Calibration targets 50 ms for the faster
implementation, constrained to 10,000 iterations and 512 MiB total source work.
Its one-document estimate includes first-use costs, so this is a target, not a
guaranteed batch duration. Allocator caches warm within batches while parser
contexts remain fresh. Six calibrations reached the iteration cap; none reached
the total-source-work cap.

All **455 measured pairs / 910 rows** matched the requested work and logical
observation. Each fixture's digest also agrees across all modes and chunks.
Independent analytic counter expectations cover all nine shapes.

The table shows **libxml2 median / yaxml median** at chunk 8,192. Values above
one favor yaxml; values below one favor libxml2. DOM cells exclude observation,
while streaming cells include it, so compare implementations within a column.

| Shape | Borrowing DOM | Owned DOM | Slice reader | Push | Buffered pull |
| --- | ---: | ---: | ---: | ---: | ---: |
| Tiny | 2.684 | 2.031 | 3.135 | 1.245 | 1.935 |
| Flat | 0.722 | 0.645 | 0.784 | 0.361 | 0.576 |
| Text | 0.182 | 0.180 | 0.531 | 0.478 | 0.538 |
| Attributes | 1.123 | 0.868 | 0.943 | 0.623 | 0.821 |
| Namespaces | 0.633 | 0.545 | 0.866 | 0.447 | 0.672 |
| Normalized | 0.962 | 0.882 | 1.046 | 0.488 | 0.837 |
| Unicode | 0.733 | 0.631 | 0.923 | 0.525 | 0.733 |
| Deep | 0.834 | 0.762 | 1.271 | 0.459 | 0.728 |
| Mixed | 1.324 | 1.311 | 1.609 | 0.410 | 0.831 |

Separate phase data is essential: yaxml has a lower parse-only median in just
the two tiny DOM rows; libxml2 has the lower median in the other 16 DOM rows,
including some near ties. Cheaper Rust destruction accounts for some combined
lifecycle wins. For example, attribute-heavy borrowing DOM has parse medians
1.877 ms versus 1.515 ms, but destruction medians approximately 0.016 ms versus
0.608 ms. A combined lifecycle win must not be reported as a parse-only win.

The clearest gap is long-text DOM parsing: 500,013 input bytes take median
parse-plus-destruction time of 1.034 ms for borrowing DOM and 1.043 ms for owned
DOM, versus approximately
0.188 ms for the reference, about **5.5 times longer**. Flat push takes 2.084 ms
versus 0.753 ms, about 2.77 times longer. These are measured lifecycle gaps;
the timings alone do not identify the responsible functions.

The [65-row primary summary](validation/parse-baseline-233638a.tsv) retains
per-document min/median/max, iteration counts and ratios. Its SHA-256 is
`351a49dc06fbe8ca3f6f6211a9059b290571c1ce45af737a39340f92734e2eaa`.
The [195-row phase table](validation/parse-phases-233638a.tsv) has SHA-256
`95bec1a31c1fc1cb343f23934e6384133b27eba600d46729fe86077f5e6e8a55`.
The 5% descriptive band uses ratios above `1.05`, below `1/1.05`, and between
those bounds. It is not a statistical confidence interval.

## Verification and retained evidence

The full repository checks at `8830f74` passed **488 tests**, with nine ignored
across 40 groups, plus **11 doctests**, strict all-target Clippy and formatting.
Final clock-only corrections then passed all 12 helper/protocol unit tests and
all seven tests in the CLI binary: three actual-command tests and four shared
protocol units. This included the pinned external helper. Strict C compilation
and all-target Clippy also passed. The [holistic review](review-parse-baseline.md)
records four detected/restored faults and no remaining code findings.

The measurement recipe compared **229 tracked files** with their committed
blobs, froze source and recipe hashes, and compared all source and binary
identities after the run. It also compared **4,426 reference-source files**
before/after. Exact input SHA-256 identities, fresh archive/build settings,
calibration output, every raw row and commands are retained. Both identity and
complete-baseline sentinels were created only after their gates succeeded.

An independent raw-results audit compiled the frozen Rust fixture generator and
byte-compared all nine inputs. It verified all 910 retained rows, 65 calibrations
and exact iteration selections, 1,040 analytic counter vectors across retained
and calibration rows, sample order and stable per-fixture hashes. It regenerated
both summary tables byte-for-byte and verified all 143 pre-audit result hashes.
The audit scripts, metrics and notes are in `independent-results-audit/` under
the evidence directory.
The separate identity audit re-extracted the pinned archive and checked all
4,426 source files, all 229 committed blobs, four release binary identities,
source/recipe manifests and publication gates. Its PASS log is
`/private/tmp/yaxml-parse-baseline-audit-233638a.log`; the holistic review records
its script, hashes and exact scope.

The scripts are `/private/tmp/yaxml-parse-baseline-{run.sh,matrix.rb,analyze.rb}`.
Immutable copies, inputs, binaries, manifests and results are under
`/private/tmp/yaxml-parse-baseline-evidence-233638a`. Root checks are retained at
`/private/tmp/yaxml-parse-baseline-{all-targets,doctests,clippy,fmt}-8830f74.log`
and `/private/tmp/yaxml-parse-baseline-anchored-{cli,clippy}.log`.
The final 12-unit rerun is retained in
`/private/tmp/yaxml-parse-baseline-final-helper-units.log`.

Broader size/attribute/namespace/depth series, UTF-16/other encodings, malformed
inputs, cold latency, allocation counts, retained/peak memory and real workloads
remain open. This is not a fuzz campaign. Production source and fuzz targets
remain identical to the [prior fuzz milestone](validation-construction-fuzz-gaps.md),
whose measured 93.80% region and 94.74% branch coverage is separate evidence.
Full libxml2 compatibility, security verification and performance superiority
remain active goals.
