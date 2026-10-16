# Parser benchmark evidence

These runners measure yaxml. They do not establish a libxml2 performance win.
Build release binaries before recording runs; debug execution under
`cargo test --all-targets` is only a functional smoke check.

`cargo bench --bench parse` retains the pre-pull DOM parse/destruction, owned
parse/destruction, slice events and 8 KiB push workloads. Set
`YAXML_BENCH_ITERATIONS` to fix their batch size. Record this runner before and
after the shared-stream refactor with identical fixture bytes and settings.

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
