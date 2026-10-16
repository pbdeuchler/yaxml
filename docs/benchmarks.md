# Parser, serialization and mutation benchmark evidence

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
[mutation validation report](validation-owned-mutation.md).

## Owned structural editing

`cargo bench --bench structural_mutation` measures fresh leaf insertion,
permanent subtree removal and tombstone-heavy churn. Fixture generation and
UTF-8/UTF-16 transport encoding occur before all clocks. Each row separately
reports parse/handle setup, structural edit calls, complete live traversal,
counting-sink serialization, semantic validation plus serialization/reparse,
and owned-document destruction. Traversal touches public kinds, provenance, names, values and
attributes; churn appends and removes one fresh Text node per unit so the final
live tree stays small while permanent slots accumulate.

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

Each row also emits a `reference_model` naming the future matched libxml2
fresh-node plus `xmlAddChild`, or `xmlUnlinkNode` plus `xmlFreeNode`, lifecycle.
`comparison_status=unmeasured` is deliberate: no C helper or comparative result
is supplied. A future comparison must validate the compatible logical
projection first, include node construction and final freeing in the matching
clock, retain libxml2's Text-coalescing difference, and keep parse, traversal,
serialization and destruction policies equivalent. These yaxml-only rows do
not establish a speed or memory advantage.

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
