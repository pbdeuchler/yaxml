# Fuzzing and replay

The oracles in this directory are safe Rust with no added Cargo dependencies. They back deterministic smoke tests, a process-per-testcase replay example, and coverage-guided [cargo-fuzz](https://rust-fuzz.github.io/book/cargo-fuzz.html) (libFuzzer) targets. The release policy and coverage definitions are in [the security plan](../docs/security.md).

Run the smoke tests and replay a testcase:

```sh
cargo test --test fuzz_smoke
cargo run --release --example fuzz_replay -- fuzz/corpus/minimal.xml
cargo run --release --example fuzz_replay < testcase.xml
cargo run --release --example fuzz_replay -- --libxml2 /path/to/pinned/xmllint fuzz/corpus/minimal.xml
```

The example accepts each filename as a separate testcase, or one testcase from stdin when there are no arguments. It caps testcase size at one MiB before parsing. Syntax rejection is an expected result and exits successfully; an invariant violation panics. File/read errors and oversized input fail the process. Keep that distinction when configuring an external runner.

## Coverage-guided fuzzing

`fuzz/Cargo.toml` is a standalone cargo-fuzz package (its own workspace), so `libfuzzer-sys` never enters the library's dependency graph. Each target in `fuzz_targets/` is a thin wrapper around one `driver::exercise_*` family; `driver::exercise` composes all of them for replay and smoke tests.

| Target | Driver entry | Surface |
| --- | --- | --- |
| `dom` | `exercise_dom` | `Reader`, `Document::parse{,_bytes}`, tree/stream projection (entity markers included); a leading high-bit byte selects parser options (`fuzz/options.rs`) |
| `owned` | `exercise_owned` | `OwnedDocument` parsing and views against the borrowed DOM; a leading high-bit byte selects parser options |
| `push` | `exercise_push` | `PushParser` chunk schedules, `finish`, `read_from` |
| `buffered` | `exercise_pull` | `BufferedReader` |
| `writer` | `exercise_writer` | `Writer` event round trips, generated operations and sinks |
| `serialize` | `exercise_serialize` | `Writer::write_document` / `write_owned_document` |
| `save` | `exercise_save` | `save::to_vec` over hash-derived flags, encodings and indents, on the document and one subtree: DOM agreement, only markup an encoding lacks fails, reparsed output saves to the same bytes |
| `limits` | `exercise_limits` | Input-derived `Limits` across every parser |
| `encoding` | `exercise_encoding` | UTF-16/UTF-32 detection and `DecodeLimits` |
| `mutation` | `exercise_mutation` | `OwnedDocument::edit` operations |
| `construction` | `exercise_construction` | `OwnedDocument::create`, `append_element` |
| `names` | `exercise_names` | `Name` and owned PI targets |
| `c14n` | `exercise_c14n` | `c14n::to_string` modes, comments, node-sets and prefix lists |
| `valid` | `exercise_valid` | DTD validation over both DOMs: raw input, input wrapped as an internal subset, or an external DTD and document split at NUL; identical diagnostics, determinism, offsets and `is_valid` |
| `textreader` | `exercise_textreader` | `TextReader` node sequence, navigation, content strings, `expand`, `next` |
| `dtd` | `exercise_dtd` | DOCTYPE-wrapped input (subset, entity value, PE text) through every surface; control byte `3` serves NUL-separated fuzz resources through an in-memory resolver and checks surface agreement and load/byte budgets; a control byte with its high bit set also runs the options checks |
| `uri` | `exercise_uri` | `yaxml::uri` parse, save, escape and resolution |
| `regexp` | `exercise_regexp` | `yaxml::regexp` compile (syntax, budget) and `is_match` on a pattern line and value lines |
| `datatypes` | `exercise_datatypes` | `yaxml::schema_types` parse, compare, equality and facets from a type, second-type and facet byte and NUL-separated texts |
| `html` | `exercise_html` | `html::parse`, `PushParser` chunk independence, `html::save`/`to_html` |
| `pattern` | `exercise_pattern` | `pattern::Pattern` compile, `matches` on both DOMs, `StreamMatcher` against the DOM match |
| `xinclude` | `exercise_xinclude` | `xinclude::process` over NUL-separated pieces (including document, then resources `0`..`9`), `xpointer::evaluate` |
| `xsd` | `exercise_xsd` | `xsd::Schema::with_resolver` and `validate` on NUL-separated pieces (schema, instance, then resources `file:///f/0.xsd`..`9.xsd`) under small limits: identical diagnostics over both DOMs and on repetition, load budget held |
| `catalog` | `exercise_catalog` | `catalog::Catalog::parse`, `resolve`, `resolve_uri` and `CatalogResolver` over NUL-separated pieces (root catalog, then files `0`..`9`; the last piece also lists queries) |
| `relaxng` | `exercise_relaxng` | `relaxng::Schema::parse` and `validate` over NUL-separated pieces (schema, instance, then `include`/`externalRef` files `0`..`9`): determinism, both DOMs, reuse, offsets, load and work budgets |

```sh
cargo install cargo-fuzz
mkdir -p fuzz/corpus/dom
cargo +nightly fuzz run dom fuzz/corpus/dom fuzz/corpus -- -max_total_time=60
fuzz/coverage.sh            # all targets; or name some
```

New inputs go to the tracked `fuzz/corpus/<target>/`; the top-level seeds are read only. Crashes land in `fuzz/artifacts/<target>/`; keep a minimized reproducer in `fuzz/regressions/<target>/`; `fuzz_smoke::fuzz_regressions_replay` replays every file through that target's driver entry. `coverage.sh` replays each target's corpus plus the seeds with `-Z coverage-options=branch` on the pinned nightly `2026-06-06`, prints one `src/` TOTAL line per target, then merges all profiles into `fuzz/coverage/report.txt` (regions, functions, lines, branches per library file). `#[cfg(test)]` modules (the DTD work) are not compiled into the targets and are absent from the denominator. The "mismatched data" warning comes from the per-binary libFuzzer entry symbols, not library code.


`fuzz/corpus/<target>/` is tracked (about 20 MiB, 33k files): the `cargo fuzz cmin` set after seeding and fuzzing. Regenerate with:

```sh
fuzz/seed.py [XMLCONF_DIR LIBXML2_TEST_DIR]   # uv run; skips inputs over MAX_INPUT_BYTES
for t in dom owned push buffered writer serialize save limits encoding dtd c14n xpath uri; do
  (cd fuzz && cargo +nightly-2026-06-06 fuzz run $t -- -fork=1 -ignore_crashes=1 -ignore_timeouts=1 -max_total_time=900 -rss_limit_mb=2048 -timeout=60)
done                                          # <= 8 at a time
(cd fuzz && cargo +nightly-2026-06-06 fuzz cmin $t -- -timeout=60)   # then delete files over 16 KiB to bound size
```

`seed.py` feeds the W3C XMLTS and libxml2 `test/` trees to the XML targets (dom, owned, push, buffered, writer, serialize, limits, encoding), DOCTYPE-leading documents and `.dtd`/`.ent` bodies (control byte 0) to `dtd`, `selector byte + XML` (15 selectors for `test/c14n`, 2 otherwise) to `c14n`, XPath expressions alone and NUL + the matching `XPath/docs` file to `xpath`, and `reference\nbase\n/:%#` lines from `test/URI` to `uri`. `mutation`, `construction` and `names` use structured bytes and are fuzzed from the top-level seeds only. Crashes found while seeding are in [`open/`](open/README.md) (unfixed) or `regressions/` (replayed by `tests/fuzz_smoke.rs`). The `dtd` target runs only owned, serialize, push and DOM checks (about 1,000-2,000 exec/s, versus 50-145 when it ran every family).

`xinclude` (2026-10-19): 331 s on `nightly-2026-06-06` with `-timeout=2`, about 2.8 million runs (8,400 exec/s), 9,413 coverage edges, no crashes or timeouts; the merged 3,184-file corpus (1.8 MiB) is tracked in `corpus/xinclude/`.

`pattern` (2026-10-19): 421 s on `nightly-2026-06-06` with `-timeout=2`, about 5.5 million runs (13,100 exec/s), 3,147 coverage edges, no crashes or timeouts; the 2,774-file `cmin` corpus is tracked in `corpus/pattern/`.
`catalog` (2026-10-19): two 331 s runs on `nightly-2026-06-06` with `-timeout=2` (the second on the final code: about 1.6 million runs, 4,800 exec/s, 6,518 coverage edges), no crashes or timeouts; the `cmin` set of 2,975 files (2.4 MiB) is tracked in `corpus/catalog/`.
`relaxng` (2026-10-19): two 331 s runs on `nightly-2026-06-06` with `-timeout=2` (the second on the final code: about 1.8 million runs, 5,500 exec/s, 16,429 coverage edges), no crashes, timeouts or leaks; seeded from libxml2's `test/relaxng`, `spectest.xml`, `testsuite.xml` and `xsdtestsuite.xml` cases; the `cmin` set without files over 4 KiB (5,293 files, 2.4 MiB) is tracked in `corpus/relaxng/`.

Merged `src/` coverage from the tracked corpora (`fuzz/coverage.sh`): 88.73% regions, 83.97% functions, 91.13% lines, 83.61% branches (previously 70.72% regions with ignored, unseeded corpora). Weakest files by region: `xpath/mod.rs` 54%, `origin.rs` 77%, `tokenizer/local.rs` 78%, `dtd_frames.rs` 79%, `stream/dtd.rs` 82%, `dtd_controller*.rs` 82%.


## Current target inventory

| Implemented input surface | Exercise path | Oracle or invariant |
| --- | --- | --- |
| `Error`, `EditError`, `ReadError`, `WriteError` display | Actual parser, editor and I/O failures through `driver::check_display` | Complete deterministic message; formatter rejection at the first write and before the final byte propagates `fmt::Error`; successfully written prefixes match the full message |
| `Reader::new`, `with_limits`, `read_event`, `position`, `event_range`, `event_origin` | `driver::exercise`, `exercise_with_limits` | Bounded forward progress; terminal EOF/error; valid error offsets and event spans |
| `Document::parse_bytes` | `driver::exercise` | Acceptance matches complete reader consumption; structural invariants |
| `Document::parse` | `driver::exercise`, for valid UTF-8 | Acceptance matches byte input |
| `Document::with_limits` | Derived profiles and each explicit resource ceiling | Policy acceptance matches the reader; each limit has a known rejection case |
| `OwnedDocument::parse`, `parse_bytes`, `with_limits`, `with_decode_limits` | `owned::exercise` and `encoding::exercise`, default and derived profiles | Temporary source overwritten/dropped before views; complete values/topology/spans match borrowing DOM or independently decoded reference; aggregate and decoded ceilings |
| Owned tree views and navigation | Successful owned trees | Every kind, declaration, name, attribute, PI target, payload and navigation method compared; exact iterator lengths; valid lookup and foreign-identity rejection |
| `OwnedDocument::edit`, `Editor::document`, `set_node_value`, `set_attribute_value`, `append_text`, `append_cdata`, `append_comment`, `append_processing_instruction`, `remove_subtree`, owned handle lookup | `mutation::exercise`, independent operation and quota selectors | Exact predicted errors; live child-vector model; fresh/stale handles; provenance; persistent quotas without removal refunds; recovery; Text-run serialization projection |
| `OwnedDocument::create`, `Editor::append_element`, construction specs/prefix policies and typed error locations | `construction::exercise`, independent bounded namespace catalog and input-derived payload/limit selectors | Exact lexical and expanded names, ordered attributes/declarations, complete snapshots, provenance, persistent quotas, foreign/stale handles, atomic rejection, binding/name-limit precedence and long inherited-prefix growth |
| `PushParser::new`, `default`, `with_limits`, `feed`, `finish`, `position`, `event_range`, `event_origin` | Input-derived chunk and lifecycle schedules, capped at 4096 chunks | Acceptance and exact typed event/span parity with slice reader; monotonic consumed position; repeated EOF; sticky errors after feeding final EOF |
| `PushParser::read_from` | Cursor plus bounded scheduled and invalid-count readers | Same events/spans; one Interrupted retry; original injected I/O error kind/message; cancellation stops reads; terminal parser states perform no I/O; oversized reported counts are rejected |
| `SourceOrigin::input_range`, `Node::origin`, `OwnedNode::origin` | Callback metadata and existing range projections across parsing, owned construction and mutation | Exact original-byte ranges remain independent of normalized/edited values; parsed and constructed provenance stays distinct; range accessors exercise origin getters transitively |
| DOM traversal and node access | Successful document checks | Unique IDs; preorder traversal; parent/child/sibling agreement; nested source ranges; subtree sizes |
| `Node::text_chunks`, `OwnedNode::text_chunks` | Reader-event projection, parsed owned views and the independent mutation model | Document-order Text/CDATA chunks; own comment/PI leaf values; no descendant comment/PI inclusion; empty/adjacent chunk preservation; stale removed content excluded |
| `Writer::new`, `with_limits`, `write_event`, `finish`, `position`, `into_inner` | Accepted reader events, generated caller-created event sequences and scheduled sinks | Exact typed round trips; complete output reparses; malformed operations emit no bytes; finite encoded bounds; confirmed partial positions; sticky failure; no I/O after failure or repeated successful finish |
| `Writer::write_document`, `write_owned_document` | `serialize::borrowed`, `serialize::owned`, UTF-8 and independently decoded UTF-16 references | Complete tree/declaration comparison; exact output and quota goldens; fresh-writer requirement; short/interrupted/invalid writes, prefix and flush faults; successful completion and terminal misuse |
| XML normalization and references | Reader/DOM corpora and generated documents | Exercised indirectly; semantic golden tests are also required in the ordinary test suite |
| Validated name construction and owned PI targets | Every bounded testcase enters `driver::exercise_names` before XML/encoding dispatch | 28 literal QName/offset goldens, borrowed/owned/Cow parity and buffer preservation, arbitrary bounded strings, getter recomposition, and exact owned-PI Writer/Reader bytes; `name-owned-views.txt` seeds a Unicode QName |
| `yaxml::uri` parse, raw parse, display, escape, unescape, normalize, resolve, relative | Every bounded UTF-8 testcase enters `uri::exercise` (reference, base, escape list as lines); `uri.txt` seeds | No panics; raw parse is ASCII and reparses; normalization idempotent; escape/unescape round trip up to NUL |
| `yaxml::regexp::Regex::new`, `with_budget`, `is_match`, `as_str`, `Clone` | `regexp::exercise` (pattern line, value lines); `corpus/regexp` seeds from libxml2 `result/regexp` | Syntax offsets inside the pattern; only budget or syntax failures; more budget compiles the same language; warm and fresh caches agree; matches contain only XML characters; `(P)*` accepts "" and doubled matches; `(P)\|literal(v)` accepts `v` |
| `yaxml::schema_types::Type::parse`, `normalize`, `Value::compare`, `equals`, `Facet::new`, `Facet::check` | `datatypes::exercise` (type, second type, facet bytes; value, value, facet value); `corpus/datatypes` seeds | Normalization idempotent and verdict-preserving; list items valid; reflexive comparison and equality; antisymmetry for decimal, boolean and binary order; enumeration of a value accepts it; whiteSpace always passes |
| `html::parse`, `html::PushParser`, `html::save`, `html::to_html`, HTML `OwnedDocument` views | Every testcase enters `html::exercise`; options, small limits and a 1–17 byte chunk schedule derive from an input hash; `html.html` seeds | No panics; only limit/unsupported-encoding failures; push equals pull (output, diagnostics, stop); tree links, depth, merged non-empty character data, valueless attributes read as ""; saved output reparses |
| `xpath::XPath::compile_with_limits`, `evaluate`, `Context` namespace and limit bindings | `driver::exercise_xpath` (also entered from `driver::exercise`): expression bytes, optionally followed by NUL and an XML document; small finite limits | Borrowed and owned DOMs agree exactly at the root and document element (value, error kind, node ranks); node-sets strictly in document order; source text round-trips |
| `c14n::to_string`, `Options` (mode, comments, node-set, inclusive prefixes) | `driver::exercise_c14n` (also entered from `driver::exercise`): one selector byte (mode, comments, one of eight fixed XPath selections, prefix list) followed by an XML document | Borrowed and owned DOMs agree exactly (output or error); whole-document output that reparses is its own canonical form |
| `TextReader::new`, `read`, `next`, node getters, `get_attribute*`, `move_to_*`, `read_string`, `read_inner_xml`, `read_outer_xml`, `expand` | `driver::exercise_textreader` (also entered from `driver::exercise`): one control byte (bit 0: one-byte `BufRead`; rotated per node: navigation and content checks) followed by an XML document; `textreader.xml` seeds | Node sequence equals the slice `Reader` event projection under libxml2's node rules, with equal acceptance; movement agrees with indexed and named lookup; `read_string` matches the subtree text; outer XML reparses and equals `expand`; inner XML children stand alone; `next` at every node visits exactly the top-level nodes |
| `pattern::Pattern::compile_with_limits`, `matches`, `stream`, `StreamMatcher::push`, `push_attr`, `pop` | `driver::exercise_pattern` (also entered from `driver::exercise`): flags byte, pattern, optionally NUL and an XML document; `fuzz/corpus/pattern/` seeds | Borrowed and owned DOMs agree on every element; in pattern mode (flags 0) each streamed element and attribute result equals the DOM match; other modes must not fail |
| `xinclude::process`, `Options` (base/lang fixup, depth, limits), `xpointer::evaluate` | `driver::exercise_xinclude` (also entered from `driver::exercise`): the including document, then NUL-separated resources an in-memory resolver serves as `d/0`..`d/9`; the last piece is also a pointer; `fuzz/corpus/xinclude/` seeds | Deterministic results and requests; load/byte budgets respected; no `xi:include` left; the result writes, reparses with equal elements and text, and stays within the node limit; fixup options never turn success into a non-limit error; both DOMs locate equally many XPointer nodes |
| `catalog::Catalog::{parse, resolve, resolve_uri}`, `catalog::Limits`, `CatalogResolver` | `driver::exercise_catalog` (also entered from `driver::exercise`): an XML or SGML root catalog, NUL-separated files an in-memory resolver serves as `d/0`..`d/9`, fixed queries plus the last piece's lines; `fuzz/corpus/catalog/` seeds (libxml2 `test/catalogs`, delegates, cycles, SGML) | Deterministic answers and requests across rebuilt catalogs; load/byte budgets respected; `resolve(None, None)` is `None`; `CatalogResolver` requests exactly the URI a twin catalog maps |
| `PushParser::with_decode_limits`, UTF-8/UTF-16 detection, declared single-byte decoding | `encoding::exercise`, `exercise_decode_limits`, encoded fixtures/mutations, `charset-*.xml` | Standard-library UTF-16 reference, xmllint-recorded single-byte tables, declaration agreement, exact typed events and original source spans, split-independent acceptance, sticky failures and decoded ceilings |
| `BufferedReader::new`, `with_limits`, `with_decode_limits`, `read_event`, `position`, `get_ref`, `into_inner` | `pull::exercise`, explicit resource/decode profiles, deterministic pull selectors | Lending event/range parity with the independent slice/UTF-16 reference; complete EOF sequence; small valid input must succeed; exact consume ledger, unread suffix, bounded caller copies, original injected I/O error and terminal no-I/O behavior |

Additional option combinations, allocation failures, and future constructors require new cases before this inventory can claim complete execution coverage. Existing-element attribute insertion/removal, moves, namespace editing, entities, queries, schemas, and foreign binding registries require targets when implemented. A target that calls an API does not establish that all its branches execute.

Owned trees use the same normalization budget for pool growth and parsing. They
can reject a valid borrowed tree with OwnedBytes, or reach that ceiling before a
later syntax failure. All other errors and successful trees are compared
explicitly. For accepted inputs up to 1024 bytes under default limits, owned
success is required: with DTDs unsupported, a conservative parser/pool capacity
bound is `16*n*(n+64)`, at most 17 MiB here, below the 64 MiB default. This check
prevents the OwnedBytes exception from hiding blanket rejection; it must be
revisited when entity expansion or storage accounting changes. Default byte and string constructors run sequentially, dropping the
first owned tree before the second. Complete comparisons borrow pool views
rather than cloning repeated namespace URIs. Canonical owned trees check
foreign document identities; generated Unicode/normalization cases force
pool growth, and tiny aggregate limit profiles exercise parser/storage sharing.
Independent budget goldens require minima of 1, 3, 8 and 10 bytes for `<r/>`,
`<r>a</r>`, `<r>&amp;</r>` and `<r a='&#9;'/>` respectively once the reader accepts.
These check full replacement-capacity charges as well as parser/storage sharing,
so accepting extra documents after undercounting does not evade fuzz assertions.

Streaming comparisons preserve exact logical token boundaries and raw byte spans. `tests/stream.rs` checks every split of small valid/malformed fixtures, all prefixes, single-byte and deterministic random schedules, normalization goldens, resource boundaries, and cancellation at every event. The fuzz driver adds one bounded input-derived schedule per testcase, ordinary `Cursor` I/O, and a custom reader that interrupts once before either delivering bytes or injecting an error at a chosen read. It verifies error kind/message/source and the exact last allowed read after XML failure or cancellation. Arbitrary-input comparisons advance a borrowed slice reader inside each callback: they do not clone expanded namespace URIs into snapshots. Owned captures in `tests/support/capture.rs` are restricted to bounded ordinary fixtures. These checks do not measure process memory or replace retention tests with a discard callback.

## Writer operation format

Recognized UTF-16/UTF-32 signatures take a dedicated path in `fuzz/encoding.rs`.
The bounded harness independently decodes code units using `char::decode_utf16`,
requires BOM/declaration agreement, and rewrites only a matching declaration's
encoding value to UTF-8 for reference grammar validation. It preserves the
original declaration for comparison and independently maps reference spans back
to source bytes. Materializing decoded input and a global offset map is confined
to this test oracle; production decoding retains constant carry. A second BOM is
explicitly rejected rather than letting the UTF-8 reference strip it. Both
auto-decoding surfaces must reject malformed code units, XML and labels.
A BOM-less declaration whose label the owning parser resolves to a single-byte
table (probed byte by byte) also takes this path; the probed table must equal a
row of `tests/fixtures/charsets.tsv`, which then decodes the reference, and an
unmapped byte leaves no reference. EBCDIC signatures are rejected like UTF-32.
For independently decoded malformed grammar, error kinds and mapped source
offsets are compared across whole/chunked/I/O/owned paths. Upfront character
validation, allocation and resource ordering have scoped exclusions. Incomplete
second-root markup or top-level CDATA can report streaming EOF instead of the
slice boundary error; both must still identify the same source start.

Successful UTF-16 input compares complete stream/I/O events and owned-tree views
against the UTF-8 reference. Small valid inputs must succeed under default
budgets, preventing blanket rejection behind resource exceptions. Larger inputs
can reach owned/decoded-token ceilings. Derived source, owned and decoded profiles
check offsets, sticky errors and split-independent acceptance. Every UTF-8 input
also exercises derived decoded ceilings. Writer operation/sink generation still
runs for encoded inputs; their retained UTF-16 declaration is not blindly copied
to the UTF-8 writer. Generated fixtures cover both orders, optional/matching BOMs,
surrogates, normalization, malformed declarations, truncations and mutations.

Every testcase also enters `fuzz/writer.rs`. Accepted XML events are serialized with an encoded byte ceiling of `6 * input.len() + 1024`, then compared in lockstep with the original reader and reparsed by DOM and push APIs. The writer may exhaust its owned-string budget earlier than the borrowing slice reader; this outcome is explicitly checked for terminal failure. Escaping can expand output, so reader input/token ceilings are raised for the reparse. Raw markup values supplied by the reader have already undergone XML normalization.

Independent operation generation interprets seven bytes as policy/I/O controls, the next byte's low five bits as an operation count (capped by available bytes), then up to 31 operation bytes and at most 96 bytes of shared logical payload. Each operation selects an event kind, name-field inconsistencies, and declaration/PI variants. The low bit of the first byte selects default versus derived limits; defaults here cap output at 16 KiB and a token at 4096 bytes. Completed sequences must reparse; rejected events leave the Vec sink position unchanged and poison subsequent calls. Deterministic seeds cover accepted nested sequences as well as invalid operations and values.

Reparsing alone cannot detect a raw delimiter that changes the caller's value into other legal markup. The operation oracle separately requires rejection for forbidden comment/CDATA/PI delimiters and inconsistent qualified/prefix/local fields, XMLNS declaration metadata, and namespaces on unprefixed ordinary attributes. Full namespace-scope rejection still also relies on the parser and ordinary independent semantic goldens.

The first two bytes independently select a small fixed valid output operation's sink schedule: short writes, one write/flush interruption, zero/oversized counts, a failure after a chosen prefix, immediate failure, or flush failure. Checks preserve the original I/O kind/message/source, confirmed output position and no subsequent I/O. Recovered sink panics are covered by ordinary tests, not the fuzz target, since the replay harness treats unexpected panics as crashes. No output-performance claim follows from these checks.

Complete-document serialization has a separate oracle in `fuzz/serialize.rs`.
Both borrowed and owned helpers serialize accepted trees, reparse the entire
output and compare every node kind, name, namespace, ordered attribute, payload,
PI target, declaration and navigation link. Node counts and complete DOM parsing
prevent a matching prefix from concealing omitted or extra output. Comparisons
borrow views rather than retaining namespace snapshots. Encoding labels become
`UTF-8`; declaration absence, version and standalone remain exact. Source ranges,
top-level whitespace and the lexical distinction between `<r/>` and `<r></r>`
are not retained by the DOM. Empty CDATA, comments and PIs remain distinct nodes.

The output ceiling is `6 * reference.len() + 1024`; reparse input/token ceilings
are raised accordingly. A valid reference of at most 256 bytes must serialize
successfully with default string budgets. Larger inputs may exhaust the
cumulative owned-string budget, but only that specific error is allowed and it
must poison the writer. This restriction prevents blanket OwnedBytes rejection,
including on owned UTF-16 trees. A prior empty Text event must consume freshness;
reusing a completed document writer must fail without further I/O.

Seven small canonical shapes have independent literal output and below/exact/
above ceilings for output bytes, encoded token bytes, output event count,
attributes, depth and cumulative owned-string bytes. In particular,
`<r><a>x</a><a>x</a></r>` requires three owned bytes and `<r>&amp;</r>` requires
six; these catch per-token resets and duplicate normalization charges. Every
shape runs through borrowed, owned UTF-8 and both UTF-16 orders, with deterministic
sink selectors independent of grammar bytes. Scheduled sinks compare accepted
prefixes directly with the expected output and preserve injected error identity,
confirmed positions and the final flush. Arbitrary outputs up to 4096 bytes also
run an input-selected sink schedule. Recovered sink panics, deep iterative
traversal, retained capacity and allocation-failure evidence require ordinary
tests and measurement in addition to these fuzz assertions. `serialize.xml`
combines declaration rewriting, unused namespace declarations, inherited
prefixes, namespace resets, normalization, empty lexical nodes and epilog nodes.

The same input selects a default constructor and terminal operation sequence. Successful parsing is followed by either an empty/nonempty feed after final EOF or direct I/O consumption of the finished parser. Failed states are also exercised through `read_from` with a source that panics if touched. An input-selected invalid-count reader reports more bytes than its output buffer holds and must produce `InvalidData` after exactly one read. These operations reuse states reached by the testcase; no fixed canonical document is parsed on every invocation.

Acceptance comparisons allow streaming to exhaust its owned-byte budget earlier because it retains names that the slice parser borrows. Arbitrary malformed inputs compare acceptance rather than identical first-error precedence: slice construction validates the complete input first, while streaming validates arriving tokens. Successful inputs require exact values, namespaces, attribute order, event kinds, and spans, with no text coalescing.

`tests/fuzz_smoke.rs` replays every seed and every prefix, replaces every seed byte with delimiter/boundary bytes, deletes every seed byte, generates deterministic arbitrary bytes, and generates accepted namespace-aware nested documents. These tests check invariants and some known acceptance outcomes. Reader/DOM agreement alone is weak because they share a parser; the structural and semantic regression checks remain necessary.

## Rooted construction model

[`construction.rs`](construction.rs) runs for every bounded testcase before
encoding dispatch, including non-XML bytes. It predicts results from separate
literal namespace cases rather than copying the production prefix planner.
Input byte zero selects one of six root catalogs; byte one selects the parsed
parent encoding; byte two selects one of four inherited-scope cases; byte three
selects one of eight exact rejection/quota cases. Derived names and values use
at most eight input bytes. Mandatory creation, append/edit/remove interactions,
repeated borrowed strings and persistent-counter checks prevent blanket rejection.

The model compares lexical and expanded names separately, ordered attributes
and declarations, stable handles, complete topology, source provenance, exact
errors/locations and unchanged snapshots after rejection. Serialize/reparse is
an additional check rather than its sole oracle. UTF-8 and both UTF-16 orders
exercise parsed parent scopes. Sparse selectors also check bounded formatting
and a 512-to-4,336-byte inherited prefix, including exact `NameBytes` failure
and successful retry. The latter forces scratch growth with a small child spec.
Private reservation failpoints remain ordinary-test evidence; fuzz-only profiles
keep allocation and other unexecuted errors in their coverage denominator.
Three child catalogs extend these checks. `construction_validation.rs` has 42
literal invalid/control pairs, each exercised through create and append. Bytes
four and five select a family and variant; byte six selects create or append.
`construction_namespaces.rs` uses byte six modulo 28 for namespace cases and
byte seven for parsed-parent encoding. Its ordinary tests enumerate every
append case in UTF-8 and both UTF-16 orders. `construction_limits.rs` uses bytes
eight and nine for hand-counted namespace-work, name/item, depth, persistent
quota, component-byte and aggregate-byte boundaries, including first-error
precedence. `construction_validation.rs` additionally uses bytes seven and eight
for a 4-surface by 3-variant prefix-compatibility and `NameBytes` precedence
catalog. Missing control bytes are zero. These are bounded independent examples,
not complete namespace or quota state-space exploration.

`construction-controls.bin` supplies independent selector bytes. The
`construction-binding-limit.bin` seed selects the UTF-16BE compatible control
with bytes seven and eight equal to 3 and 2. The format seed starts with `0x2a`;
the four-byte long-prefix seed is `[0x2b, 0xff, 2, 3]`, forcing the 4,336-byte
prefix with a UTF-16BE parent. All four run through actual replay dispatch and
the ordinary corpus mutation schedules. Test-only counters check that empty,
malformed, sparse-selector and encoded inputs enter construction and the binding
catalog before grammar/encoding returns.

The [original construction validation](../docs/validation-owned-create.md) and
[current fuzz-gap evidence](../docs/validation-construction-fuzz-gaps.md) record
separate source-complete measurements and remaining gaps.
The [binding/limit precedence validation](../docs/validation-construction-binding.md)
records the twelve-case catalog, dispatch/encoding fault checks and its fresh
fuzz-only measurement without production changes or exclusions.

## Owned mutation operations

[`mutation.rs`](mutation.rs) runs before the driver's UTF-16 early return,
whether or not the testcase itself is XML. Two ten-node documents have manually
specified names, namespaces, ordered attributes, values and child vectors. They
contain every leaf kind, Unicode, empty lexical nodes, inherited/shadowed bindings
and a default namespace reset. Source bytes are overwritten and dropped before
editing. Moving the containers must preserve saved handle ownership.

Model IDs increase monotonically. Each ID keeps a live flag and ordered children;
removal unlinks a subtree and marks every descendant dead. Neither model IDs nor
production slots may be reused. Parsed survivors retain their original source
ranges, fresh leaves have no source range, and removed handles must fail lookup
and later edits. Traversal follows model children rather than production slot
order, links or subtree counts.

Both documents must accept a small valid operation through each value setter.
The first document must also accept all four append methods and a document-level
comment, then permanently remove a created comment. These mandatory operations
prevent blanket rejection from passing before arbitrary records are interpreted.

### Header and records

Missing header bytes read as zero. Header bytes `H0` through `H7` control:

| Byte | Interpretation |
| --- | --- |
| `H0` | Low bit selects the first seed family; the second uses the opposite family. |
| `H1` | Low two bits select UTF-8, UTF-8 BOM, UTF-16LE BOM or UTF-16BE BOM, in that order. The second document uses the next mode modulo four. |
| `H2` | Bit 0 enables per-string byte ceiling `H3`; bit 1 enables created-node ceiling `H5 & 63`. Otherwise each uses its default. |
| `H3` | Derived per-string byte ceiling, from 0 through 255. |
| `H4` | `H4 & 7` selects the value-quota family; `(H4 >> 3) & 7` selects the structural-quota family below. Exactly `42` (`*`) also runs the namespace/lexical byte golden. |
| `H5` | Boundary selector modulo three, and derived created-node ceiling when enabled. |
| `H6` | Bit 0 requests same-editor recovery after rejected operations; bit 1 selects reopened editors in the repeated value-quota sequence. |
| `H7` | Low five bits request up to 31 complete eight-byte records, capped by bytes available after the header. |

At most 256 bytes after the records form shared payload. General sequences keep
the default cumulative owned-byte ceiling. Value and creation ceilings can vary
independently; opening below already-spent creation work must reject. Bounded
valid operations must succeed: OwnedBytes and Allocation are not exceptions for
general sequences.

Record bytes are `R0` through `R7`. `R0 & 15` selects the operation; `R1 & 1`
selects the current document container. Most operations select a saved handle
with `R2 % saved.len()`. The initial registry contains both parsed trees, first
document then second. Successful append records add returned handles; removed
handles remain selectable.

| Opcode | Operation and remaining controls |
| --- | --- |
| 0 / 1 | Replace node / existing attribute value. Attribute selection uses `R3 % 12`. |
| 2 | Lookup the saved handle. If `R6` bit 0 is set, replace saved entry `R7 % saved.len()` with current-document handle `R2 % current.handles.len()`, including stale handles. |
| 3 | Serialize and compare the complete tree with the model projection. |
| 4 | Open an editor with header limits; compare acceptance with spent creation work. |
| 5 | Swap document containers and models without changing handle owners. |
| 6 / 7 | Foreign node / attribute replacement. Select `R2 % foreign.handles.len()` from the other owner instead of the saved registry. |
| 8 / 9 / 10 / 11 | Append Text / CDATA / Comment / PI to the saved parent. PI target selection uses `R3 % 16`. |
| 12 | Permanently remove the saved subtree. |
| 13 | Add current-document handle `R2 % current.handles.len()` to the saved registry, including stale handles. |
| 14 | Reopen with created ceiling selected by `R6 & 3`: zero, spent-minus-one saturating at zero, spent, or spent-plus-one. Later records still use header limits. |
| 15 | Force a foreign structural operation. `R3` bit 4 selects removal; otherwise append the kind selected by `R4 & 3` in Text/CDATA/Comment/PI order. |

For replacements and appends, `R4`/`R5` select payload start/length, clamped to the
shared payload. `R6 % 16` selects the value: 0 uses lossy UTF-8 decoding, 1 is empty,
12 is `valid`, and 15 repeats `🙂` `R7 & 31` times. Other variants supply Unicode,
forbidden XML characters, delimiters, CR or leading whitespace. PI target variant
0 uses the same lossy slice; 9 is `é🙂` and 12 is `p`. Other targets include empty,
reserved `xml`, invalid Name characters and valid Unicode names. The complete
variant and expanded-attribute selector tables are the `value`, `pi_target_value`
and `selector` functions in [the decoder](mutation.rs). Invalid UTF-8 never
reaches an `&str` mutation API.

The model predicts exact errors before production, including PI argument order
and UTF-8 byte indices. Append success publishes one fresh model ID; removal
invalidates the independently computed descendant set. After each operation,
checks compare all live values, names, namespaces, ordered attributes,
declaration, handles, provenance, navigation, traversal order and exact iterator
lengths. Dead handles must remain unresolved. Recovery uses the same editor for
a known-valid empty comment or existing-value edit; ordinary records open fresh
editors. The model reuses no production validation or pool accounting.

### Literal quota families

Quota fixtures have independent expected acceptance, not a replica of pool
growth. Existing value families exercise full-capacity charges, equal-value
appends, spare capacity, Unicode byte limits and editor reopening. For
`<r>x</r>`, one-byte replacement needs three owned bytes; a ceiling of ten allows
four replacements and rejects the fifth. For `<r a='x'/>`, two-byte replacement
needs five bytes, while one byte fits parsed spare capacity for zero new owned
bytes.

Structural families use `H5 % 3` to select these below/exact/above profiles:

| Family | Fixture and expected boundary |
| --- | --- |
| 0 | `<r/>`, append empty Text with created ceilings 0/1/2 and zero owned bytes: reject/accept/accept. |
| 1 | `<r/>`, append Text `x` with owned ceilings 1/2/3: reject/accept/accept. |
| 2 | `<r/>`, append PI target `p`, data `x`, with owned ceilings 2/3/4: reject/accept/accept. A rejected PI leaves zero-budget reopening and subsequent Text growth probes unchanged. |
| 3 | Create/remove an empty comment, then reopen with created ceilings 0/1/2. Zero rejects opening; one rejects the next append; two accepts empty CDATA. Removal refunds nothing. |
| 4 | Create PI `p`/`x` for three owned bytes and remove it. Next Text `x` needs a full request of four: cumulative ceilings 6/7/8 reject/accept/accept. Lower reopened ceilings and a recovery append check both counters. |
| 5 | `<r a='x'/>`, append one Text byte using parsed spare capacity at zero owned bytes, then remove it. Creation ceiling one still rejects a new node; raising it to two accepts an empty comment. |
| 6 | PI target and data each equal `é🙂`; per-string ceilings 5/6/7 reject/accept/accept. Their combined length is not the per-string limit. |
| 7 | At zero creation/owned ceilings, invalid PI targets/data and invalid CDATA report earlier semantic errors; valid Text reaches CreatedNodes. |

All families run in UTF-8 and both UTF-16 orders in deterministic smoke tests.
They target quota resets/refunds, delta-only or duplicate full-capacity charges,
partial PI staging and creation work spent on rejection.

### Serialization and corpus

Final and selected intermediate trees serialize through `write_owned_document`
and reparse completely. The only model projection drops empty Text and combines
each maximal adjacent Text run by concatenating values. In-memory Text nodes
never merge. CDATA never joins a Text run; empty CDATA, Comment and PI remain
distinct. An empty Text child can emit `<e></e>` while reparsing drops that child
and later emits `<e/>`, so byte idempotence is not assumed. Output is capped at
64 KiB and encoded tokens at 16 KiB.

`mutation-operations.bin` and `mutation-quotas.bin` retain value-edit coverage.
[`structure-operations.bin`](corpus/structure-operations.bin) adds 13 explicit
structural records with header `00 02 00 00 20 01 00 0d`. It constructs the first
tree through UTF-16LE, appends nonempty and empty Text beneath the nested element,
and serializes their Text-run projection. It then appends empty CDATA, a
document-level comment and a Unicode-target PI; removes the nested subtree;
checks a saved new descendant is stale; rejects a stale append and a foreign
removal; reopens at the exact creation count; swaps containers and serializes.
The header also selects structural family 4's exact cumulative pool profile.
All three operation seeds are registered for ordinary prefix and byte-mutation
smoke tests. Replay this seed directly with:

```sh
cargo run --release --example fuzz_replay -- fuzz/corpus/structure-operations.bin
```

Public callers cannot forge slots. Invalid slots and identity exhaustion use
private tests. Safe thread-local node/pool reservation failpoints are also
private unit-test controls, absent from normal replay and fuzz-smoke library
builds. Simulated allocation errors and rollback tests are separate evidence;
fuzz assertions do not exercise actual allocator exhaustion. Element/attribute
construction, existing-node moves and namespace editing remain outside this slice.

The [structural libxml2 oracle](../docs/oracle-structure.md) reparses a fixed
catalog fixture and performs exactly one operation per helper invocation. It
compares compatible C/Rust tree projections and selected bytes, and classifies
intentional differences such as Text coalescing and root removal. It does not
execute modeled multi-operation sequences or their saved/stale handle and quota
protocol against libxml2. The parsing/serialization `--libxml2` replay comparison
is a separate oracle as well.

## Source coverage

Pull schedules derive a capacity from 1/2/3/7/64/8192/65536, an early-stop event
count from the low three control bits, and an I/O-failure byte boundary from the
full control byte. Ordinary replay derives that byte from the testcase midpoint;
the deterministic selector test varies it independently of XML grammar bytes.
Large cases raise the smallest visible slice to `ceil(input length / 4096)`;
per-event fill/consume calls can still exceed 4096. A second complete run exposes
the entire remaining input, so buffer
capacity cannot change the final XML result. One source retries an interruption;
another fails at an exact input boundary, including EOF. Source operations are
counted and rejected after terminal states; `consume` cannot exceed the exposed
slice. Source extraction must agree exactly with the parser's consumed position.
Recovered source panics remain ordinary-test cases: unexpected fuzz panics are
crashes, not swallowed outcomes.

The pull oracle compares live events rather than cloning namespaces. It copies
at most 32 payload scalars for the caller-retention operation. Default cases of
at most 1024 input bytes cannot conceal rejection behind an owned-budget
exception. Four canonical tiny documents have independent minima for raw input,
raw token, cumulative retained/normalized strings, events, attributes and depth.
Their below/exact/above profiles run through ordinary suspension. The existing
independent decoded-total/token profiles also exercise pull, including cases
which must succeed at the exact ceilings. UTF-16 values, raw spans and semantic
error offsets reuse the standard-library reference, not the shared push engine.

The `pull-lookahead.xml` corpus seed combines text/markup suspension, empty
CDATA, PI, references, inherited prefixes and a default-namespace reset. Every
prefix and byte mutation is replayed by the existing corpus tests. Retained-state
plateau and source-panic checks require ordinary private/public tests in addition
to the fuzz assertions; a source-region percentage alone cannot establish them.

CI instruments `fuzz_smoke` and uploads raw profiles, merged profiles, a compiler version, a revision, a library-only summary, and the complete LLVM coverage export. The summary excludes dependency/toolchain code and the test/driver source so its denominator is library code. The complete export retains those files for inspection. No production library regions are excluded by this filter. The run exercises only the default feature set and only deterministic fuzz smoke tests; it is separate from ordinary unit-test coverage.

Coverage uses pinned nightly `2026-06-06` with `-C instrument-coverage -Z coverage-options=branch`. The unstable branch instrumentation is documented in the [Rust Unstable Book](https://doc.rust-lang.org/unstable-book/compiler-flags/coverage-options.html). Its branch denominator covers the conditions instrumented by that compiler; it does not enumerate every parser state or boolean condition. The library itself remains stable Rust. See [measured results and gaps](../docs/validation.md).

Use LLVM tools from the matching Rust toolchain's `llvm-tools-preview` component. A system LLVM version can reject profiles from a newer Rust compiler. If tooling is unavailable, record that coverage was not measured; do not infer it from passing tests. The workflow publishes gaps and currently imposes no percentage threshold. A release requires reviewed coverage data and the additional gates in the security plan.

## Differential testing

`--libxml2 PATH` requires libxml2 **2.15.4** and records its version/build report.
`--allow-other-libxml2` explicitly selects an exploratory comparison with another
version; it does not change the target. Tree and XPath support are required.
Accepted fixtures compare complete, independently evaluated XPath values through
the [bounded infoset protocol](../docs/oracle.md); oracle values never pass through
yaxml's parser. Once the original comparison succeeds, the complete-document
helper serializes the tree and a second invocation evaluates those bytes with
the original query and expected values. UTF-8 uses the borrowing document helper;
UTF-16 uses the owning helper. Semantic agreement requires both comparisons.
Projection or serialization storage ceilings remain explicitly counted
acceptance-only outcomes. This external projection merges text/CDATA and omits
declaration/XMLNS metadata; the stricter in-process tree comparison above covers
those fields. Rejected fixtures compare acceptance. All invocations use
`--nonet --nocatalogs --strict-namespace --nowarning`, with both catalog variables
explicitly empty. Warnings such as forward-version notices are suppressed;
namespace errors remain fatal through `--strict-namespace`.

Each oracle invocation has a five-second execution deadline and an 8 MiB combined
stdout/stderr ceiling (16 KiB for its version report). Regular-file capture avoids
pipe deadlocks. Output-size polling can overshoot temporary disk usage; this is
not a disk quota or a sandbox for an untrusted executable. Timeout/output failures
kill and reap the direct child. File operations and arbitrary descendants are
outside that termination guarantee. Capture directories use best-effort cleanup.

DTD-containing bytes, unsupported encodings, declared-encoding mismatches, invalid UTF-8 and yaxml limit
failures are policy skips. The DTD byte prefilter is conservative and can also
skip that spelling inside a comment. Projection ceilings produce separately
counted acceptance-only results. A clean exit 0 is acceptance; exit 4 is rejection;
other statuses and successful exits with diagnostics are inconclusive. Both
disagreements and inconclusive results fail the command. Summaries distinguish
semantic agreement, rejection agreement, acceptance-only, policy skip,
inconclusive and disagreement. [xmllint options and return codes](https://gnome.pages.gitlab.gnome.org/libxml2/xmllint.html).

CI is configured to download the checksum-pinned release, build a static external
oracle, compare the corpus, and retain metadata. Local availability and actual
hosted execution must accompany any result; configuration alone is not evidence
of a pinned pass. The comparison does not establish identical resource policy,
error categories, recovery or diagnostic wording. Minimize disagreements and
retain their classification. No external XML implementation is a Cargo dependency.
