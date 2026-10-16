# Implementation roadmap

The first useful release is a bounded XML reader and DOM with precisely stated compatibility. A full libxml2 replacement follows in stages. Feature presence, behavior compatibility, source compatibility, and C ABI compatibility are separate deliverables; the feature inventory must track each explicitly. The core stays pure Rust, forbids unsafe code, and depends only on `memchr` throughout.

## Priority and acceptance gates

| Priority | Work | Why this order | Required evidence |
| --- | --- | --- | --- |
| P0 | UTF-8 XML 1.0 Fifth Edition slice reader; namespaces; structured errors; finite limits; arena DOM; normalization and references | Establish one semantic engine and ownership model before multiplying interfaces | Accepted/rejected grammar tests; limit boundaries; reader/DOM parity; target inventory and corpus replay; documented unsupported constructs |
| P0 | Compatibility harness, corpus management, coverage reporting, benchmark runner | Measure behavior, security, and performance before tuning representation | Reproducible commands and raw results; pinned libxml2 oracle; no unsupported “100%” or performance claims |
| P1 | Buffered and push streaming; SAX-style event adapter; owned input/document mode | Make large-document use practical and support callback-oriented bindings | Chunk-split equivalence; temporary/final EOF; bounded retained memory; I/O error and cancellation tests |
| P1 | XML writer/serializer; UTF-16 input/output and encoding detection; basic tree mutation | Complete common XML workflows and close the UTF-8-only conformance gap | Parse/write semantic round trips; malformed operation tests; encoding boundary corpus; namespace repair specification |
| P1 | Stable Rust API review and binding boundary prototype | Avoid committing language bindings to unstable node ownership | Lifetime/handle tests; thread and callback contract; explicit resolution of ABI/unsafe constraints |
| P2 | DTD declarations, entity representation/expansion, defaults, IDs and DTD validation; resolvers and catalogs | Common legacy XML workflows, with substantial new security exposure | Capability-based resolution; aggregate budgets; entity cycles/amplification tests; validation corpus and differential results |
| P2 | XPath 1.0 and pattern selection; Canonical XML; XInclude; URI utilities | High-value document processing built on stable tree and resolver semantics | W3C suites where available; explicit data model; query/output budgets; canonical byte comparisons |
| P3 | HTML parser/serializer with documented libxml2 dialect; RELAX NG; XML Schema 1.0; Schematron | Independent grammars and large compatibility surfaces need dedicated workstreams | Versioned compatibility matrices and suites per subsystem; dedicated fuzz targets and limits |
| P3 | XPointer, XML 1.1, remaining legacy encodings, Unicode regex/automata, debug/tooling compatibility | Complete the selected libxml2 baseline after core workflows stabilize | Symbol/behavior inventory burndown; explicit exceptions for removed/deprecated behavior |
| P4 | Python, Ruby, JavaScript/TypeScript, and Go packages; migration tools; any approved ABI adapter | Distribute a core whose contracts have settled | Per-language ownership, error, concurrency, streaming, packaging and integration tests |

These priorities order implementation, not importance. Security and measurement accompany every row. Binding requirements influence handles, ownership, errors, and callbacks from P0 onward even though packages ship later. HTML parsing must identify the libxml2 version/dialect being reproduced rather than silently substitute browser HTML5 behavior.

## Milestone burndown

The first slice is implemented and independently reviewed; [validation evidence](validation.md)
records its scope and remaining gaps. A checked implementation item does not mean
full libxml2 compatibility, production readiness, or completion of the P0 measurement gates.

- [ ] Pin the libxml2 reference release/build features and generate a symbol inventory with per-symbol status and deviations.
- [x] Implement XML character/name ranges, declarations, document boundaries, comments, PI, CDATA, references, and attribute normalization for the UTF-8 subset; full conformance-suite validation remains pending.
- [x] Resolve namespaces, reject reserved-prefix violations and duplicate expanded attribute names, and unwind bindings correctly.
- [x] Complete bounded slice reader behavior, including sticky EOF/error and exact limit semantics.
- [x] Build DOM from the reader; verify relationships, source spans, and iterative traversal. Allocation failure injection remains an open gate.
- [x] Add a safe, dependency-free replay/mutation driver and map all implemented input APIs to it.
- [ ] Add differential comparison against a pinned external libxml2 executable and classify every disagreement.
- [x] Produce separate fuzz-only and ordinary-test coverage artifacts, with actual denominators and remaining gaps. Reaching 100% remains open.
- [ ] Record full DOM and streaming baselines for libxml2 and yaxml across a versioned corpus.
- [x] Review the initial public API and resource policy before expanding to buffered/push streaming; final API stabilization is a separate P1 gate.

The reference release is pinned and 4,051 header identifiers are indexed. A built
2.15.4 oracle, configured export verification, and per-operation Rust mappings are
still needed to close the first item. The crate has no C ABI exports.

Later rows receive their own detailed burndown when their dependencies are understood. Avoid building a generic engine for schemas, XPath, and HTML before implementing their actual semantics.

## Performance decisions

“Faster than libxml2 across the board” is the optimization target. It needs a workload matrix before it can be evaluated: tiny documents, large flat documents, deep nesting, attributes, namespaces, entity-heavy text, non-ASCII names, mixed content, malformed input, and multiple input chunk sizes. Include full DOM parse/destruction, streaming consume/discard, owned-input cost, time to first event, throughput, allocations, retained memory, and peak resident memory.

Compare release builds on the same machine with matched options, validation, limits, encoding, and event work. Record compiler, CPU, allocator, input hash, warm/cold policy, repetitions, median and spread. Parsing a borrowed input is different from copying and decoding an owned document; report both. A faster tokenizer is not a full DOM result. Run performance work in isolated experiments after correctness and security gates pass. Reject optimizations that weaken validation or turn valid input into an undocumented subset.

Use `memchr` for delimiter search when profiles justify it, borrow unchanged strings, and allocate normalized strings only when necessary. Favor contiguous arena storage and explicit stacks. Choose interning, extra indices, and compact node layouts based on measured workloads; each can trade parse time for memory or later query speed. Track adversarial scaling as well as typical throughput.

## Toolchain and allocation gate

Verify the user's Rust 1.100 allocator premise against the official issue, compiler availability, and a minimal compile probe before setting the minimum supported Rust version or making the public API generic over allocators. Standard-library allocator support, collection support, and the ability to implement a custom allocator without unsafe are separate questions. Keep the default API usable with the verified compiler, and document experimental configurations honestly. The requested reference is the [allocator API tracking issue](https://github.com/rust-lang/rust/issues/32838).

Document ownership and arena boundaries now; defer a pervasive generic allocator parameter until the toolchain gate passes and a benchmark demonstrates its value. Allocation failure and memory accounting still require audits regardless of allocator selection. An arena reduces allocation overhead; it does not bound expanded data by itself.

## References and review

Conformance follows [XML 1.0 Fifth Edition](https://www.w3.org/TR/2008/REC-xml-20081126/) and [Namespaces in XML](https://www.w3.org/TR/REC-xml-names/). The UTF-8 milestone is explicitly partial because XML processors must also support UTF-16. The [libxml2 API reference](https://gnome.pages.gitlab.gnome.org/libxml2/) defines the comparison surface; version and build features must accompany every compatibility claim. The [security plan](security.md) defines the evidence needed for fuzzing and hostile-input readiness.

Delegated work is reviewed before it reaches local main. Review the actual diff, behavioral tests, error paths, complexity, dependencies, unsafe prohibition, and public API impact. Integrate small coherent changes; update the compatibility and coverage inventories in the same change. A passing test suite establishes only its tested scope, so milestone notes must state the remaining gaps.
