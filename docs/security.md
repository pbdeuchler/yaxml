# Security and fuzzing plan

This is a development policy, not a claim that yaxml is ready for hostile production input. Safe Rust prevents classes of memory errors in this crate; it does not prevent excessive allocation, slow algorithms, stack exhaustion, incorrect XML interpretation, or mistakes in downstream bindings. Fuzzing supplies evidence about tested executions, not proof that vulnerabilities are absent.

## Trust boundaries

The Rust core forbids unsafe code and has one dependency, `memchr`. Keep the same restriction for core tests, examples, and fuzz drivers; only the standalone `yaxml-harness/fuzz/` cargo-fuzz package adds `libfuzzer-sys` and its generated unsafe entry point. Review the dependency graph and enabled features when updating the lockfile. The prohibition applies to yaxml's source; dependencies and the standard library have their own implementation and audit boundaries.

Treat document bytes, parser options, caller-created tree data, resolver responses, and binding handles as independent input boundaries. Validate bytes and XML structure before creating trusted names and nodes. A writer must validate caller-created data even when the reader already validates parsed input. Constructors and tree mutations must preserve node relationships and namespace invariants. Do not add redundant validation inside every internal function.

Use iterative stacks for input-driven nesting, including parsing, traversal, serialization, entity processing, and destruction. Avoid one heap object whose recursive ownership follows each input element. Check additions, multiplications, offsets, and conversions before using them as sizes or indices. A parser error terminates ordinary event production; diagnostic recovery, if implemented, is a separate mode with an explicit contract. XML defines fatal-error handling and requires normal processing to stop after such an error. [XML 1.0, terminology](https://www.w3.org/TR/2008/REC-xml-20081126/#sec-terminology).

The parser rejects unsupported constructs and encodings explicitly. It must not silently skip a construct and return a misleading success. Borrowing interfaces require UTF-8; owned and streaming input also accept UTF-16 under the [encoding contract](encoding.md). Decoded totals and token bytes have separate finite limits, malformed surrogates fail, and the decoder retains constant carry rather than a complete expanded source.

## Resource policy

Limits are checked before retaining more state or allocating a derived buffer.
Parsing/writing budgets span their operation; owned editing's growth counter
persists across all editors for a document. Defaults are public behavior and
need boundary tests. A compatibility option must not quietly disable unrelated limits.

| Resource | Initial enforcement | Extension required before the related feature ships |
| --- | --- | --- |
| Input | Total source bytes and total decoded UTF-8 bytes | Aggregate resolved bytes and compressed input handled outside the core |
| Tokens | Raw and decoded token bytes; constant decoder carry | Additional transcoding formats and their carry bounds |
| Owned strings | Cumulative requested string capacity, including namespace copies, streaming names and full new owned-DOM pool capacities; one shared normalization/storage counter | Allocator-level byte accounting and failure injection; current budget is not a process memory quota |
| Owned edits | Per-value/name and aggregate supplied-byte ceilings; bounded construction depth, start-tag items and namespace/expanded-name work; cumulative full pool-capacity requests, created slots and created attributes/declarations across editors; validate before storage and publish after reservation; removal refunds no counter | Existing-element attribute membership, reclamation/compaction and allocator-level failure injection |
| Structure | Element depth, attributes per element, attributes per DOM document and event count; parsed node creation is bounded by events (about 200 B each); fresh owned nodes and attributes have persistent counts; construction and subtree removal are iterative | Retained DOM bytes, existing-element namespace edits, general insertion and cycle checks, moves/import/adoption, detached ownership and slot reclamation/reuse |
| Entities | Entity nesting, reference count, expanded bytes, expansion work and cycle detection, shared by internal and external entities | Validation-time content models |
| External resources | No I/O without a caller `Resolver`; `max_external_loads` requests and `max_external_bytes` cumulative response bytes per document, checked before decoding; nesting/expansion share the entity budgets | Redirects and cancellation belong to the caller's resolver |
| Query/validation | XPath limits; DTD content models compile to Glushkov automata whose construction spends the DTD work budget and at most a quarter of the remaining DTD storage, and whose transitions are charged as storage; matching is one lookup per child; validation is iterative and allocates diagnostics per input construct; XML Schema patterns compile under a unit budget (NFA instructions, repetition copies, class ranges, 50 nesting levels) and match without backtracking through a 1 MiB DFA cache or NFA simulation; XML Schema documents load only through the caller's resolver within `xsd::Limits` (documents, cumulative bytes), content models compile under one work budget with counters instead of unrolling, particle nesting is capped, and validation tracks at most `max_counter_vectors` configurations per element | Schema state and recursion |
| Output | Encoded event and total byte ceilings, depth/attributes/events/owned strings; validation before sink I/O; iterative complete-tree serialization; partial positions and terminal errors | Namespace repair, subtree output, transcoding, canonicalization and their additional budgets |

Input size alone does not bound all work. Many attributes can cause quadratic duplicate-name checks; deeply nested namespace rebinding can cause repeated scans. Benchmark adversarial size series and record the algorithmic bound for each such path. CPU budgets should count deterministic work where possible; wall-clock deadlines belong in the embedding application or an explicit cancellation interface.

The first implementation uses a scoped namespace index and interned namespace
identities for duplicate-attribute checks. This avoids rehashing a long borrowed
URI for every attribute. Regression tests preserve aliases and scope; the
[validation report](archive/validation.md) records the adversarial probe and coverage gaps.

## Work and memory budgets

### Node and attribute memory

Every DOM node is created from one event, so `max_events` (1,000,000) is the node ceiling: a separate `max_nodes` would never fire (`OwnedBuilder` also checks `nodes.len() <= max_events` directly). Measured on release builds, one node costs about 200 bytes (1 M empty elements: 217 MB resident for the borrowed DOM and 209 MB for the owned one, including the 4 MB input and the generator's copy), so the default allows roughly 200 MB of node records. Tightening it would reject ordinary 20-30 MB data files for no new protection; libxml2 has no node ceiling at all.

Attributes were the gap. They are not events and `max_owned_bytes` charges only strings, while an element may carry `max_attributes` (1,024) of them and 64 MB of input holds 10 million `<e a="" b="" .../>`-style attributes (about 5 bytes each). A borrowed DOM keeps about 135 bytes per attribute (an owned one under 30; resident-set deltas), which is 1.3 GB for a borrowed DOM before this limit. `Limits::max_total_attributes` (default 4,000,000, `Limit::TotalAttributes`) counts every attribute a DOM retains: explicit ones, namespace declarations and defaulted attributes, summed per start tag where `Document`, `OwnedDocument` (and so `TextReader::expand`, XInclude results) store them. At the ceiling the borrowed DOM measures 678 MB resident (40 MB input included) and the owned DOM 168 MB, so a hostile document costs a bounded few hundred MB plus the 200 MB of nodes. `HUGE` does not raise it, just as it does not raise `max_events` (libxml2 has no counterpart); raise it with `Limits::max_total_attributes` when a trusted data set needs more. Streaming readers (`Reader`, `PushParser`, `BufferedReader`, `TextReader`) retain no attributes beyond the current element, so the limit is not checked there and the hot tokenizer path is untouched.

### Superlinear audit

`yaxml-harness/examples/adversarial_scaling.rs` generates the hostile families below and prints time and log-log slopes (`cargo run --release --example adversarial_scaling -- all dom 1 4 16`; `DEFAULTS=1` keeps the default limits, `FLAGS=n` sets `ParseOptions` flags, `CHUNK=n` and modes `push`/`buffered` feed n-byte chunks, `owned`, `reader`, `text`, `html` select the frontend). Every ceiling is raised for the series so the cost of the construct, not the limit, is measured. Exponent = slope of ln(time) against ln(size) from 1 to 16 MiB (apple silicon, release, borrowed DOM unless noted; owned, `Reader`, `TextReader`, `PushParser` and `BufferedReader` at 1-byte chunks agree within 0.15).

| Family (generator) | Exponent | Time at 16 MiB |
| --- | --- | --- |
| flat `<a/>` x N; tiny text/comment nodes; PIs; CDATA | 1.00-1.01 | 0.06-0.29 s |
| one element with N distinct attributes (`attrs`) | 0.99 | 0.13 s |
| 8 attributes per element (pairwise duplicate check) and 10 per element | 1.00-1.01 | 0.23-0.28 s |
| N `xmlns:pK` declarations on one element | 1.09 | 0.39 s |
| 200 in-scope prefixes, depth-50 nests using them (`deep_prefixes`) | 1.01 | 0.18 s |
| 1,000 prefixes, N attributes sharing one local name (`nsattrs`) | 0.98 | 0.11 s |
| namespace rebinding on every element (`rebind`) | 1.01 | 0.15 s |
| depth 200 repeated (`deep`) | 1.03 | 0.24 s |
| 900 KB names (`long_names`) | 1.02 | 0.02 s |
| N IDs under a DTD (`ids`); `ID`/`IDREF` validation (`idrefs`) | 0.95 / 1.01 | 0.84 / 1.04 s |
| `&amp;`/`&#65;` runs in text and attributes; whitespace-only attribute values | 1.03-1.07 | 0.04 s |
| entity references with NOENT (`entity_refs`) | 0.99 | 1.43 s |
| N `<!ENTITY>`, `<!ELEMENT>`+`<!ATTLIST>`, `<!ATTLIST>`, duplicate declarations, `%p;` chains | 0.97-1.09 | 0.30-1.19 s |
| N-token enumeration; N-name sequence content model | 1.07 / 0.98 | 0.55 / 0.98 s |
| NSCLEAN with redundant declarations | 0.99 | 0.14 s |
| NOBLANKS, whitespace between siblings | 2.0 before, 1.00 after | 0.74 s |
| DTDVALID, mixed content with 2,000 names and 1 M children | O(children x names): 13 s per 4 MB before, 1.02 after | 0.96 s |
| DTDVALID, 1,000 `#REQUIRED` attributes on every element | O(attributes x declared): 14 s per 16 MB before, 1.01 after | 1.0 s |
| mixed content or enumeration with N names (DTD parse, duplicate check) | 2.2 before, 1.0 after | 0.16 s at 6 MB |
| `(a\|b0\|...\|bN)*` content model | 1.93-2.0 | bounded by `max_owned_bytes`, below |
| 1,000 defaulted attributes per element (DTDATTR) and 1,000-node entity per reference | 1.05 / 1.0 in output size | bounded by the entity budget, below |

Fixed in this change (exponent 2 or worse, input-controlled):

- `NOBLANKS` (`xpath::tree::blanks`, shared by both DOMs): the "spoken text" test rescanned every kept sibling for each child, quadratic in the siblings of one element. A 335 KB document took 49 s, and the default 1 M events would take hours. It is now a running flag.
- DTD mixed content `(#PCDATA|a|b|...)*`: the duplicate-name check (VC: No Duplicate Types) compared each name with all earlier ones, and enumerated-attribute declarations did the same for duplicate tokens (`Dtd::declare_element`, `check_attribute`). 100 K names took 10 s. Both use a hash set.
- `DTDVALID`: each child element of a mixed-content parent scanned its declared name list, each enumerated or NOTATION attribute value scanned its token list, and each required attribute scanned the element's attributes. These are O(document x DTD), and the DTD may be 16 MB. The mixed and enumerated lists are now hash sets built on first use per validator, and required attributes are looked up in a map built once per start tag. 4 MB holding 765 K children under a 20,000-name model: 36 s before, 0.5 s after; 1,000 required attributes: 14 s per 16 MB before, 1.0 s after.

Bounded, not removed:

- A choice with N alternatives has a quadratic Glushkov automaton, as in libxml2. Construction charges the transitions to the DTD storage budget (`max_owned_bytes`) before building them, so the worst case is the time to fill that budget: about 1.5 s at the default, reached by a 21-26 KB declaration.
- Defaulted attributes with `DTDATTR` (about 100 ns each) and entity replacement text are charged to `max_entity_expansion_bytes` and the amplification rule, so a 4-byte `<a/>` cannot yield more than about five times the input consumed in copied defaults once the 1 MB allowance is spent. With defaults the 1,000-attribute case fails in 7 ms.
- XPath, regexp and content-model matching have their own work budgets (`max_operations`, pattern and automaton units).

With the default limits every family above, fed 60 MiB, finishes or fails with a limit error within 0.75 s (the borrowed DOM, same machine), except the 1.4 s content-model case, and the validation families within 1.3 s.

DTD lookups on the content path (see "DTD-heavy parsing" in [benchmarks](benchmarks.md)): entity and element names go through `Recent`, a 64-slot direct-mapped cache whose slot index is an unkeyed hash, in front of std's keyed SipHash maps. Crafted colliding names only evict slots, so the worst case is the keyed lookup plus one comparison of at most 15 bytes; no unkeyed table holds attacker-chosen keys. Linear searches are bounded by constants (ATTLISTs up to 8 entries, duplicate checks below 8 attributes). A tag charges copied defaults as the element's precomputed total less its specified attributes and walks the declarations only to copy (`DTDATTR`, namespace defaults) or note standalone defaults: `default_attrs` (1,000 defaults, `<a/>` repeated, 4 MiB) took 2.5 s and takes 0.07-0.11 s.

There is deliberately no generic CPU budget: with the fixes above, parsing time is linear in input, DTD-controlled work is bounded by byte budgets that are already enforced, and deadlines belong to the embedding application. Regression coverage: `tests/budgets.rs` (inclusive attribute ceiling on the DOM builders and its absence on the streaming readers, `HUGE` interaction, and the formerly quadratic paths under a generous time bound; the three-size series is `--ignored`).

### Defaults against libxml2 2.15.4

libxml2 defines these in `parserInternals.h`, `parser.c` and `xpath.c`; `HUGE` is `XML_PARSE_HUGE`.

| Resource | libxml2 default | libxml2 `HUGE` | yaxml default | yaxml `HUGE` | Relation |
| --- | --- | --- | --- | --- | --- |
| Text run, comment, PI, CDATA, attribute value, entity value, literal (`XML_MAX_TEXT_LENGTH`) | 10,000,000 | 1,000,000,000 (`XML_MAX_HUGE_LENGTH`) | `max_token_bytes` 1 MiB | 1,000,000,000 | Stricter (10x): one lexical token. Not a per-node limit: adjacent runs, CDATA and references can merge into a larger DOM text node (libxml2 stops a merged node at 10 MB), bounded by input and `max_owned_bytes` (64 MiB). The tokenizer buffers a token, so 1 MiB keeps streaming memory small. |
| Names, prefixes, NCNames (`XML_MAX_NAME_LENGTH`) | 50,000 | 10,000,000 (`XML_MAX_TEXT_LENGTH`) | 1 MiB (the token ceiling) | 1,000,000,000 | Looser (20x, and 100x under `HUGE`). Linear in the measured families (900 KB names, 1.02), so no separate name limit is justified; a caller who wants libxml2's value sets `max_token_bytes` or checks names itself. |
| Element nesting (`xmlParserMaxDepth`) | 256 | 2,048 | `max_depth` 256 | 2,048 | Equal. Parsing, traversal, serialisation and destruction are iterative. |
| Entity or input nesting (`ctxt->inputNr`) | 20 | 40 | `max_entity_depth` 20 | 40 | Equal (deliberately never looser than libxml2 on a recursion ceiling). |
| Entity amplification (`XML_MAX_AMPLIFICATION_DEFAULT`) | 5 | 5 | `max_entity_amplification` 5 | 5 | Equal, same rule. |
| Free expansion allowance (`XML_PARSER_ALLOWED_EXPANSION`) | 1,000,000 | 1,000,000 | `max_entity_expansion_bytes` 1,000,000 | 1,000,000 | Equal; fixed per-reference cost follows `XML_ENT_FIXED_COST` (20). |
| Parser dictionary (`XML_MAX_DICTIONARY_LIMIT`) | 100,000,000 | unlimited | none; names and normalised strings share `max_owned_bytes` 64 MiB, input is `max_input_bytes` 64 MiB | unchanged | Stricter. No global dictionary exists to grow: borrowed text is free, owned strings are charged. |
| Push-parser lookahead (`XML_MAX_LOOKUP_LIMIT`) | 10,000,000 | 1,000,000,000 | covered by `max_token_bytes` 1 MiB (a token is the unit buffered) | 1,000,000,000 | Stricter. |
| Attributes per element (`XML_MAX_ATTRS`) | 100,000,000 | same | `max_attributes` 1,024 | 1,024 | Stricter (real documents stay far below). |
| Attributes per document | none | none | `max_total_attributes` 4,000,000 | 4,000,000 | Stricter; new. |
| Nodes / events | none | none | `max_events` 1,000,000 | 1,000,000 | Stricter; documents beyond it must raise the limit. |
| Input size | none | none | `max_input_bytes` 64 MiB | 64 MiB | Stricter; the caller raises it. |
| DTD declaration | tokens above | tokens above | `max_dtd_bytes` 16 MiB | 1,000,000,000 | libxml2 has no declaration ceiling beyond its per-token ones. Content-model construction is also charged to `max_owned_bytes` and a work budget. |
| External loads, bytes | no ceiling | no ceiling | 256 loads, 16 MiB | unchanged | Stricter; loads need a `Resolver` in any case. |
| Recovered errors (`XML_MAX_ERRORS`) | 100 | 100 | 100 | 100 | Equal. |
| XPath steps / stack / node-set (`XPATH_MAX_STEPS`, `XPATH_MAX_STACK_DEPTH`, `XPATH_MAX_NODESET_LENGTH`) | 1,000,000 / 1,000,000 / 10,000,000 | same | `xpath::Limits` `max_operations` 50,000,000, `max_nodes` 10,000,000 | n/a | `max_nodes` equal; operations are a different unit (every evaluation step, not expression steps), so looser in count but a hard budget on work. |
| XPath nesting (`XPATH_MAX_RECURSION_DEPTH`) | 5,000 (1,000 on Windows) | same | `max_depth` 200 | n/a | Stricter. |

`HUGE` raises only the ceilings libxml2 raises (token, depth, entity depth, DTD bytes). Limits libxml2 does not have (`max_events`, `max_total_attributes`, `max_owned_bytes`, `max_input_bytes`) are deliberate yaxml policy and stay in force under `HUGE`.

Use fallible reservation for attacker-sized collection growth. Audit implicit allocations, string clones, formatting, and collection constructors too. Returning an allocation error from some paths does not establish that every out-of-memory condition is recoverable. Record remaining infallible allocations before claiming graceful allocation failure. Owned editing has a safe thread-local test seam that simulates returned failures at its node, attribute, pool and construction-scratch reservation boundaries; it proves those error paths and their publication atomicity, not allocator internals. Real allocator failure is injected by fuzzing, below; libxml2's fuzzers set the precedent. [libxml2 fuzzing instructions](https://raw.githubusercontent.com/GNOME/libxml2/master/fuzz/README.md).

### Allocation failure

The policy is that attacker-sized growth on the XML parsing, DOM, editing, copy and save paths reserves fallibly and fails with `ErrorKind::Allocation` (`EditErrorKind::Allocation`), while every other allocation is infallible and std aborts the process when it fails, as std does for `Vec::push` everywhere. The 99 `try_reserve` sites are in the tokenizer, parser, decoders and resolver input, the DTD stack, both DOM builders, owned editing, construction and copying, `Writer` and `save`; XPath, XML Schema, RELAX NG, Schematron, C14N, the HTML tokenizer, tree builder and serializer (an HTML document's storage is the owned DOM's), catalogs, the regexp engine and `TextWriter` have none and are bounded only by their `Limits`.

yaxml forbids unsafe code, so it cannot install a failing allocator. The `allocfail` target in yaxml-harness can: its `#[global_allocator]` over `System` fails the allocation the input's first four bytes choose while one surface runs (`tests/support/fuzz/allocfail.rs`: both DOMs with `try_clone` and `save`, the slice, push, buffered and text readers, owned edits, construction, import and copy, `Writer`, `XINCLUDE`, HTML parsing and `html::save`). The failed run must equal the unarmed run (the failure was absorbed) or fail with an allocation error, and a third, unarmed run must equal the first. An infallible allocation that hits the failure unwinds through `set_alloc_error_hook` and the run is dropped; libFuzzer's own allocations are C++ and never fail. A failing `A` for `OwnedDocument<A>` (`allocator_api`) would reach a subset of the same arms and still need `unsafe impl Allocator`, so there is no separate target.

It found three bugs, all an allocation failure reported as something else, fixed with replay inputs in `yaxml-harness/fuzz/regressions/allocfail` (replayed with `YAXML_ALLOCFAIL_SWEEP=1`, which fails each allocation in turn):

- `save` re-reads the internal subset to print it and kept the subset as written when that re-read failed for any reason, so an allocation failure produced different output instead of an error (`save/dtd.rs`).
- Under `RECOVER`, a DTD unit's recovery mapped any failure of `Budget::recover` back to the original syntax error, so a failed reservation of the recovered-error list became a recovered `InvalidDtd` and the parse went on (`dtd_controller.rs` `parse_unit`).
- The slice reader replaces a later error with the first error of its up-front validation (`cut`); it also replaced `Allocation`, so an out-of-memory parse became a recovered `InvalidUtf8` with a truncated tree (`reader.rs` `read_event`).

Infallible input-sized allocations. With `YAXML_ALLOCFAIL_MIN_SIZE=16384` the target fails every allocation of 16 KiB or more; over 22 generated families of 30-100 KiB documents (text, elements, attributes, names, comments/PIs/CDATA, namespaces, entities and parameter entities, ATTLISTs, IDs, content models, enumerations, depth, external subsets, whitespace, recovery, invalid UTF-8, UTF-16, Latin-1, `xml:base`) on both DOMs (with `save`) and the four readers, under seven option sets, the parsers and both DOM builders made no infallible allocation of that size: each either succeeded or returned `Allocation`. The infallible ones it hit, each of which aborts the process when the allocator refuses:

- `save` output: `Out` grows its buffer with `extend_from_slice` (`save.rs` `Out::raw` and the text escapers); the output is about as large as the document.
- `TextReader`: the node queue (`VecDeque::push_back` in `fetch`) that `read_outer_xml` and the other subtree calls fill; it grows with the buffered subtree, up to `max_owned_bytes`.
- DTD validation: the IDREF list (`valid/engine.rs`, `references.push`) grows with the number of IDREF attributes.

The audit stops at the first infallible allocation of each run, so it lists what is reached first, not every site. Everything past the policy line (the subsystems above without fallible reservations, `Writer`/`TextWriter` output, diagnostics, `html::save`) is infallible by construction; moving any of them under the fallible policy is a design change, not a bug fix.

## External resources and compatibility

No document-controlled filesystem or network access is implicit. External DTD subsets and entities load only through a `Resolver` the caller passes in; XInclude uses the same path (`ResourceKind::XInclude`, `XIncludeText`), with its loads, bytes and result size bounded across the whole call; XML Schema includes, imports and redefines too (`ResourceKind::Schema`, never `xsi:schemaLocation` unless the caller asks with `Schema::from_instance`). Catalogs never load implicitly: there is no default catalog, no `XML_CATALOG_FILES` and no catalog processing instruction. A `catalog::Catalog` is built from caller bytes or one resolver request, fetches `nextCatalog`, delegate and SGML `CATALOG` files lazily through that resolver (`ResourceKind::Catalog`) within its own load, byte and nesting limits, and `CatalogResolver` only rewrites the URI the inner resolver is asked for, so the inner resolver's policy still decides access ([catalogs](catalog.md)). The core sends a `Request` with the resource kind, the system ID resolved against the declaring resource's base, the public ID and the remaining byte budget; the caller decides access policy. A resolver can return in-memory bytes without granting ambient filesystem access. Limits are cumulative across recursive loads, never per resource, and loaded text shares the document's entity expansion, reference and depth budgets, so external "billion laughs" stops at the same ceilings. Entity and default-attribute expansion is bounded by libxml2's amplification rule (a fixed allowance, then at most `max_entity_amplification` times consumed input), with a fixed cost per use; see the [compatibility notes](compatibility.md#document-type-declarations).

`FileResolver` is an opt-in convenience: it serves relative paths and `file:` URIs beneath one canonicalized root, rejects lexical `..` escapes, symbolic links leaving the root, non-file schemes and non-regular files, declines missing files and reads at most one byte past the remaining budget. Its checks precede `open` and are not atomic with it, so it is not a defense against a concurrently modified directory tree. It never touches the network.

Keep resource policy distinct from syntax compatibility. libxml2 documents that entity substitution can load external entities, DTD validation can be vulnerable to complexity attacks, and its `NONET` option is not a general restriction on custom loaders. A security profile cannot be implemented by copying that single flag. [libxml2 parser options](https://gnome.pages.gitlab.gnome.org/libxml2/html/parser_8h.html#xmlParserOption).

Bindings require a separate design review. Opaque handles must reject stale and foreign-document identifiers; ownership and release must be explicit; strings need lengths and an encoding contract; panics must not cross a foreign boundary. Raw-pointer ABI glue cannot be promised under a whole-project unsafe prohibition. Resolve the binding mechanism and policy before implementing it, without weakening the core's prohibition by accident.

Owned lookups and edits reject foreign-document handles through a checked,
non-recycled document identity. Value editing preserves topology. Fresh element/leaf
insertion creates stable handles without source ranges. Permanent subtree
removal makes every removed handle stale while preserving surviving handles and
provenance; slots are never reused or reclaimed. The API does not expose
detached nodes or implement existing-element attribute insertion/removal, moves, import/adopt,
cycle-forming operations, release registries or a foreign ABI. Failed appends
remain usable and preserve logical state, though allocation requests already
charged can remain spent. [Owned edit contract](owned-dom.md#editing-existing-values)
and [structural edit contract](owned-dom.md#leaf-insertion-and-permanent-removal).
Complete rooted construction stages names, initial attributes and declarations
without exposing an invalid intermediate tree. Aggregate supplied bytes bound
caller-string validation; a separate namespace-work budget bounds ancestor scans
and expanded-name comparisons, including long inherited bindings. Scratch is
input/work-bounded and reserved fallibly. Exact-prefix intent is gathered before
automatic choices so a later name cannot retarget an earlier name's namespace.
The [construction evidence](archive/validation-owned-create.md) and
[subsequent fuzz-gap audit](archive/validation-construction-fuzz-gaps.md) retain
unexecuted error paths in their fuzz-only denominators. Independent validation,
quota and namespace catalogs now enter through every bounded replay input;
complete execution coverage remains open.

## What “100% fuzzing coverage” means

The [complete XMLTS catalog run](archive/validation-xml-conformance.md) is separate
conformance evidence. Every case and parser interface has an explicit result,
including unsupported and unscored rows. A per-ID/interface pass baseline makes
previously supported behavior becoming a capability gap a failing regression.
The trusted metadata oracle can expand descriptor entities; testcase parsing
does not gain external-resource access. This acceptance run neither replaces
fuzzing nor contributes to the reported fuzz-only execution percentage.

Maintain three separate reports. Never describe one as the others.

1. **Target coverage: 100% required.** The denominator is every implemented public input-bearing API and every implemented parsing, normalization, encoding, query, validation, mutation, and writing subsystem. Each entry maps to at least one executable fuzz target, its input format, invariants, options, and seed corpus. New APIs extend the denominator in the same change. An API exercised only by unit tests is not fuzz-covered.
2. **Measured execution coverage: 100% is the target, with gaps visible.** Report functions and LLVM source regions reached by fuzz-corpus replay separately from regions reached by ordinary tests. Include library code for every supported feature configuration, including error and allocation handling. Report both raw totals and any reviewed exclusions; do not exclude uncovered code to improve a percentage. Collect branch outcomes with a separately validated compiler configuration when available. Region coverage is not branch coverage.
3. **Behavior coverage: an enumerated checklist.** Record exercised grammar productions, parser states, transitions, error categories, options, and resource-limit outcomes. State-transition and input-space coverage do not have a finite universal “100%” denominator. Publish the actual inventory and known gaps.

### Measured coverage (2026-10-20)

`yaxml-harness/coverage.sh` over every target and its committed corpus (plus `tests/fixtures/fuzz-seeds`), nightly-2026-10-18, aarch64-apple-darwin, default features, `-Z coverage-options=branch`, `src/` only:

| | Regions missed | Functions missed | Lines missed | Branches missed |
| --- | --- | --- | --- | --- |
| Before (core `cdc363b54`, 30 targets) | 2,273 of 45,542 (95.01%) | 262 of 4,520 (94.20%) | 1,294 of 36,729 (96.48%) | 540 of 8,358 (93.54%) |
| After (31 targets with `allocfail`) | 993 of 45,551 (97.82%) | 46 of 4,520 (98.98%) | 541 of 36,735 (98.53%) | 455 of 8,384 (94.57%) |

The earlier 97.58% figure predated later public API (`chars`, `features`, DOM accessors, `reset`, `LineIndex::owned`, ...) that no target called; at `cdc363b54` nine committed inputs also failed (now in `yaxml-harness/fuzz/open`) and were set aside for the baseline. Allocation failure was 319 of the 2,273 missed regions, not most of them; the rest were unfuzzed API, missing driver calls and unexplored input. The work: the `allocfail` target, drivers for the uncalled API (both DOMs' libxml2 accessors compared, `reset`/`reset_with` replays, `chars`, `features`, `LineIndex::owned`/`describe_all`/`column`, validators' `describe`, `TextReader` attribute values and properties, `CATALOG_PI` across readers, `set_url`, `OwnedNodeId::parts`, an XSD model-work sweep that checks the budget is monotone), and a 300 s plus a 600 s campaign of every target with the new finds merged edge-only into the corpora.

What is left, by class:

- Allocation failure: 4 regions, three `try_reserve` sites reachable only past states the surfaces rarely reach: `owned/copy.rs` staging kept forms and HTML valueless flags for a copy (an HTML document is not an editing surface), `owned/construction.rs` namespace scratch growth.
- Reachable, missed: 905 regions. Mostly error propagation from a budget that has to run out at one exact step (`?` arms in XSD content-model construction and fix-up, owned editing, copy and construction under `EditLimits`, the diagnostics re-reader's `Work`), `TextWriter` misuse sequences, XPath tree accessors, `xs:date`/`duration` overflow, and DTD, XInclude and resolver corner cases. Per file: xsd/fixup 68, owned/edit 42, xsd/automaton 37, textwriter 34, owned/copy 33, textreader 33, xsd/validate 32, owned 31, xpath/tree 31, owned/construction 30, parser 25, diagnostic 24, tokenizer/text 23, dtd_catalog 23, diagnostic/reread 21, xsd/idc 21, schema_types/temporal 20, xinclude 20, dtd 19, schema_types 19, dtd_controller/content 18, save 17, resolve 17, c14n 13, stream/dtd 12, and 10 or fewer in each of 43 more files. `stream/dtd.rs`'s `Failure::Poisoned` arm needs a panicking callback, which libFuzzer's panic hook turns into a crash; unit tests cover it.
- Unreachable or defensive: 84 regions, each kept for a reason:
  - Compile-time only: `html/tables.rs` `el` (a `const fn` evaluated into the tables) and `setters!`'s `every_field_has_a_setter`, which exists to fail compilation.
  - `unreachable!` already states the impossibility: `dtd_controller/content.rs` (5, source variants), `owned.rs` `Content::Removed`, `owned/edit.rs` text merge, `parser.rs` `boxed` and `emptied` (closures a type conversion needs), `reader.rs` declaration handoff, `stream/dtd.rs` checked phase.
  - A `Vec` sink cannot fail: `save::to_vec` and `html/serialize.rs` `to_vec` map `WriteError::Io` (each 2).
  - Event kinds a caller never passes: `owned.rs` and `parser.rs` conversions of declarations or ends, `writer.rs` `encode_event` for entity boundaries (`write_event_inner` drops them first).
  - Origins and tokens the reader guarantees: `dom.rs` and `owned.rs` (`event_origin` after an event), `stream.rs` (2 sites), `stream/dtd.rs` `lend_plain`, `textreader.rs` `fetch` at EOF, `tokenizer/text.rs` progress check, `encoding.rs` `check_progress` (the decoder's consumed-bytes contract).
  - Counters that cannot overflow: document and generated-prefix identifiers (`owned.rs`, `owned/construction.rs` 2, `owned/copy.rs`; 2^64 documents, or a work budget that ends first; unit tests preset the counter), namespace ids and binding counts (`parser.rs` 3, one per declaration), offsets and usage sums (`tokenizer/reference.rs` 2, `tokenizer/text.rs` 2, `dtd_reference.rs`, `dtd_catalog.rs` 2), all bounded by the input or by limits far below `usize::MAX`. The `parser.rs` ones are written `checked_add(1).ok_or(Allocation)?`; replacing them with `+= 1` would be shorter and behave the same, but changes overflow handling in the parser and is left to review.

The coverage artifact records revision, compiler/LLVM version, target platform, features, optimization flags, corpus revision, execution count, elapsed time, unique failures, and exclusions. Rust supports LLVM source-based coverage instrumentation and external LLVM reporting tools; those tools are build infrastructure, not crate dependencies. [rustc coverage guide](https://doc.rust-lang.org/rustc/instrument-coverage.html).

## Fuzz targets and oracles

| Target family | Inputs and checks |
| --- | --- |
| Slice reader | Arbitrary bytes, limit profiles, repeated calls after EOF/error; terminate without panic; offsets remain inside source; names and references obey XML rules |
| DOM | Arbitrary bytes; agree with the reader on grammar acceptance; valid parent/child/sibling links; iterative traversal visits exactly the stored nodes; owned storage survives input destruction and preserves complete views/ranges with explicit earlier OwnedBytes exhaustion |
| Normalization | Text/attribute/reference contexts; legal scalar references; distinguish literal whitespace from referenced whitespace; preserve documented normalization semantics |
| Streaming | Same bytes across every split for small cases and generated schedules for larger cases; exact typed event/span agreement on success; distinguish feed from final EOF; sticky cancellation, I/O faults and no further reads after terminal failure |
| Writer | Parsed-event sequences plus complete borrowed/owned trees, including UTF-16-owned input; strict tree round trips, emitted-event and encoded ceilings, fresh/terminal states, partial/Interrupted/invalid-count/erroring sinks, and no I/O after terminal results |
| Owned-value mutation | Independent operation model predicts errors and values; compare complete state after every success/failure, foreign handles/moves, persistent quotas and literal output goldens; serialize/reparse with only documented empty/adjacent Text projection |
| Owned leaf structural mutation | Independent operation model predicts exact append/remove results; check placement, lexical rules, stale/foreign/protected handles, stable survivors, all reciprocal links, subtree sizes, source provenance, persistent quotas and serialized Text projection after every operation |
| Rooted document/element construction | Independent finite namespace catalog with derived payloads/limits; predict exact names, ordered declarations/attributes and errors before calls; compare complete topology/provenance, persistent counters, failure snapshots and long inherited-prefix handling across UTF-8/UTF-16 parents |
| General structural mutation, when implemented | Reject cycles and illegal namespace/ownership transitions; bound existing-element attribute/namespace edits, sibling insertion, moves, import/adoption, reclamation and slot reuse |
| Advanced features, when implemented | Separate grammar targets plus resolver, query, schema, entity, and cancellation operation sequences; assert budgets and deterministic results |

Differential testing runs a separately built, version-pinned libxml2 executable outside the Rust dependency graph. Match options and limits before comparing. Compare acceptance and a documented semantic projection of events/trees; attribute order, text event segmentation, diagnostics, and recovery require explicit rules. A disagreement is a triage item, not automatic proof that either implementation is correct. Check the W3C specification and add the minimized case to regressions. Namespace duplicate checks must compare expanded names, and unprefixed attributes have different namespace rules from elements. [Namespaces in XML](https://www.w3.org/TR/REC-xml-names/).

Use both arbitrary byte mutation and grammar-generated valid documents. Seed all lexical constructs, Unicode/name boundaries, CR/CRLF, references, malformed delimiters, namespace shadowing, duplicate attributes, truncation at every byte, and each limit at zero, below, equal, and above its boundary. Add deep/wide documents and many shared-prefix names to test complexity. Keep every minimized bug as a deterministic regression with its invariant, not just as an opaque corpus file.

The dependency-free driver backs a stdin/file replay example, deterministic smoke tests, and one cargo-fuzz (libFuzzer) target per input family; `yaxml-harness/coverage.sh` reports per-target and merged library coverage. Random mutation without coverage feedback is a smoke test, not evidence of coverage-guided fuzzing. See [yaxml-harness/fuzz/README.md](https://github.com/pbdeuchler/yaxml-harness/blob/main/fuzz/README.md).

## Release gates

Every implementation milestone must have regression tests for accepted and rejected behavior, corpus replay, resource-limit tests, target-inventory updates, and a coverage artifact. Before a production release, require the raw fuzz-only function and region coverage reports, branch gaps, independent differential results, allocation audit, dependency audit, and adversarial complexity results. Any unreachable region needs a written justification or removal. A percentage below the stated goal must be reported as a gap, not rounded to completion.

Continuous fuzz infrastructure, sustained run budgets, private vulnerability reporting, supported release policy, and independent security review remain release prerequisites to establish. No elapsed fuzzing duration alone proves security. Security fixes need a minimized regression, affected-version assessment, and a release note that describes the actual impact.
