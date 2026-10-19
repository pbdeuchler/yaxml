# Implementation roadmap

The first useful release is a bounded XML reader and DOM with precisely stated compatibility. A full libxml2 replacement follows in stages. Feature presence, behavior compatibility, source compatibility, and C ABI compatibility are separate deliverables; the feature inventory must track each explicitly. The core stays pure Rust, forbids unsafe code, and depends only on `memchr` throughout.

## Priority and acceptance gates

| Priority | Work | Why this order | Required evidence |
| --- | --- | --- | --- |
| P0 | UTF-8 XML 1.0 Fifth Edition slice reader; namespaces; structured errors; finite limits; arena DOM; normalization and references | Establish one semantic engine and ownership model before multiplying interfaces | Accepted/rejected grammar tests; limit boundaries; reader/DOM parity; target inventory and corpus replay; documented unsupported constructs |
| P0 | Compatibility harness, corpus management, coverage reporting, benchmark runner | Measure behavior, security, and performance before tuning representation | Reproducible commands and raw results; pinned libxml2 oracle; no unsupported “100%” or performance claims |
| P1 | Buffered and push streaming; SAX-style event adapter; owned input/document mode | Make large-document use practical and support callback-oriented bindings | Chunk-split equivalence; temporary/final EOF; bounded retained memory; I/O error and cancellation tests |
| P1 | UTF-8 event writer; DOM serialization helpers; namespace repair and formatting | Complete common output workflows with explicit namespace and failure contracts | Parse/write semantic round trips; invalid-event rejection; encoded-output limits; partial I/O and panic tests; namespace repair specification |
| P1 | UTF-16 input/output and encoding detection; basic tree mutation | Close the UTF-8-only conformance gap and support editing workflows | Encoding boundary corpus; parse/write semantic round trips; malformed operation tests |
| P1 | Stable Rust API review and binding boundary prototype | Avoid committing language bindings to unstable node ownership | Lifetime/handle tests; thread and callback contract; explicit resolution of ABI/unsafe constraints |
| P2 | DTD declarations, entity representation/expansion, defaults, IDs and DTD validation; resolvers and catalogs (catalogs implemented, see [catalog.md](catalog.md)) | Common legacy XML workflows, with substantial new security exposure | Capability-based resolution; aggregate budgets; entity cycles/amplification tests; validation corpus and differential results |
| P2 | XPath 1.0 (implemented, see [xpath.md](xpath.md)) and pattern selection; Canonical XML (implemented, see [c14n.md](c14n.md)); XInclude and XPointer (implemented, see [xinclude.md](xinclude.md)); URI utilities | High-value document processing built on stable tree and resolver semantics | W3C suites where available; explicit data model; query/output budgets; canonical byte comparisons |
| P3 | HTML parser/serializer with documented libxml2 dialect; RELAX NG; XML Schema 1.0; Schematron | Independent grammars and large compatibility surfaces need dedicated workstreams | Versioned compatibility matrices and suites per subsystem; dedicated fuzz targets and limits |
| P3 | XML 1.1, remaining legacy encodings, Unicode regex/automata, debug/tooling compatibility | Complete the selected libxml2 baseline after core workflows stabilize | Symbol/behavior inventory burndown; explicit exceptions for removed/deprecated behavior |
| P4 | Python, Ruby, JavaScript/TypeScript, and Go packages; migration tools; any approved ABI adapter | Distribute a core whose contracts have settled | Per-language ownership, error, concurrency, streaming, packaging and integration tests |

These priorities order implementation, not importance. Security and measurement accompany every row. Binding requirements influence handles, ownership, errors, and callbacks from P0 onward even though packages ship later. HTML parsing must identify the libxml2 version/dialect being reproduced rather than silently substitute browser HTML5 behavior.

## Milestone burndown

The first slice is implemented and independently reviewed; [validation evidence](archive/validation.md)
records its scope and remaining gaps. A checked implementation item does not mean
full libxml2 compatibility, production readiness, or completion of the P0 measurement gates.

- [ ] Pin the libxml2 reference release/build features and generate a symbol inventory with per-symbol status and deviations.
- [x] Implement XML character/name ranges, declarations, document boundaries, comments, PI, CDATA, references, and attribute normalization for the UTF-8 subset; full XML conformance remains open.
- [x] Resolve namespaces, reject reserved-prefix violations and duplicate expanded attribute names, and unwind bindings correctly.
- [x] Complete bounded slice reader behavior, including sticky EOF/error and exact limit semantics.
- [x] Build DOM from the reader; verify relationships, source spans, and iterative traversal. Allocation failure injection remains an open gate.
- [x] Add a safe, dependency-free replay/mutation driver and map all implemented input APIs to it.
- [x] Add spec-derived acceptance/value regressions across slice, DOM and push input; correct forward-version and encoding-name declaration grammar.
- [x] Run the complete pinned W3C XMLTS 20130923 catalog across all five parser
  interfaces: 2,585 cases, 12,925 classified rows. Preserve the supported subset
  with a 407-ID/1,871-pair regression baseline. The
  [validation report](archive/validation-xml-conformance.md) records zero mismatches
  within the scored slice and substantial remaining DTD/entity/version gaps;
  it does not claim full-suite conformance.
- [x] Add bounded independent XPath infoset comparisons, strict oracle version checks and a checksum-pinned external-oracle CI build.
- [x] Run pinned external libxml2 comparisons for the 18-file replay corpus and
  34-case structural catalog, with every result classified. Full conformance
  and broader multi-operation differential campaigns remain open.
- [x] Produce separate fuzz-only and ordinary-test coverage artifacts, with actual denominators and remaining gaps. Reaching 100% remains open.
- [x] Add a bounded coverage-guided mutation campaign with replayable lineage,
  per-case execution/profile limits, strict coverage-schema checks and retained
  failure evidence. The [500-case validation](archive/validation-fuzz-campaign.md)
  establishes the harness mechanics, not sustained security assurance.
- [x] Triage remaining fuzz-only gaps and add independent finish-cancellation,
  BOM-less encoding-rejection and pre-root quota assertions. The
  [fresh measurement](archive/validation-fuzz-gap-triage.md) reaches 4,859/5,082 regions
  and 658/692 instrumented branch outcomes; 100% remains open.
- [x] Record the first [matched DOM and streaming baseline](archive/validation-parse-baseline.md)
  for the implemented UTF-8, DTD-free profile: nine shapes, five APIs, 65
  workloads and 455 retained pairs. All observations agree; libxml2 has the
  lower primary median in 42 workloads, yaxml in 19, with four within the
  descriptive band. Performance superiority remains open.
- [ ] Extend the parsing baseline to size/attribute/namespace/depth series,
  UTF-16 and remaining encodings, malformed inputs, cold latency, allocation
  counts and retained/peak memory; profile and close the measured parser gaps.
- [x] Profile long unchanged text and replace scalar normalization detection
  with `memchr`, removing the duplicate check while preserving borrowing,
  quotas and diagnostics. The [paired Rust validation](archive/validation-normalization-scans.md)
  records about 1.78–1.79× faster large-text DOM parsing and 1.37–1.38× faster
  large-chunk streaming. All 65 workloads retain exact observations; a tiny
  outlier did not repeat in 31 samples. Full XMLTS results remain unchanged.
  Fresh fuzz-only coverage is 6,808/7,260 regions and 913/964 branch outcomes;
  100% and performance superiority over libxml2 remain open.
- [x] Establish [owned-capable names and PI targets](archive/validation-name-ownership.md)
  for future parser-owned replacement input: validated private QName storage,
  constant-time getters/views, independent ownership and Unicode tests, and
  fuzz dispatch. Name, Attribute, Element, Event and borrowing NodeKind are each
  sixteen bytes smaller on the measured compiler. Complete XMLTS results remain
  unchanged. Fresh fuzz-only coverage is 6,878/7,331 regions and 909/962 branch
  outcomes, including all instrumented outcomes in `name.rs`. The original
  performance matrix retains repeated streaming/deep regressions; controlled
  helper outlining removes the seven-byte push difference without library
  changes. This needed functionality is accepted with documented performance
  debt; the benchmark remains unchanged. DTD acceptance and 100% remain open.
- [ ] Establish original-input versus replacement-source provenance across
  events, diagnostics and both DOMs before activating the remaining
  [DTD foundations](archive/plans/dtd-foundations.md). Preserve physical stream position,
  UTF-16 source mapping, bounded expansion and explicit external-resource policy.
- [x] Introduce the limited [source-origin metadata boundary](archive/validation-source-origin.md)
  across Reader, Push, Buffered and both DOMs, with optional exact physical spans,
  absent/sticky last delivery, explicit cancellation endpoints and fallible DOM
  joins. All 535 tests and 13 doctests pass; complete XMLTS results remain
  unchanged. Fresh fuzz-only coverage is 6,936/7,406 regions and 910/964 branch
  outcomes, with new defensive failures retained in the denominator. Reader/Push
  grow eight bytes; node records/Event stay unchanged. The 65-workload Rust A/B
  run has no median slowdown above two percent. Real replacement provenance and
  DTD acceptance remain open; the [representation outline](archive/plans/dtd-representation.md)
  preserves the broader internal-DTD/GE/defaults milestone.
- [ ] Integrate the complete internal-DTD controller and semantics. Reviewed
  private [lexical-unit](archive/plans/dtd-lexical-unit.md) and
  [incremental-scanner](archive/plans/dtd-scanner.md) prototypes now pass 50 focused
  groups and the combined 585-test/13-doctest suite. The
  [prototype review](archive/review-dtd-prototypes.md) records independent catalogs,
  deliberate faults and the 56-byte unit layout. Their source remains on
  `integrate/dtd-prototype`, outside main. Reviewed private
  [binding/default semantics and entity frames](archive/review-dtd-semantics.md) now
  extend that foundation to 129 focused groups and 664 tests/13 doctests.
  Public integration of the controller, replacement/mixed provenance, DOM
  coalescing, events and serialization, plus fuzzing, conformance and performance
  gates remain open.
  The later [private controller/provenance review](archive/review-dtd-controller.md)
  joins incremental PE processing, shared occurrence provenance, parse-only Text
  joins and phase-aware literal output. Its subsequent private borrowed adapter
  moves validated events without cloning and preserves the catalog, frame nonce,
  counters and distinct decoded/physical body-resume positions. The combined
  freeze passes 788 tests and 13 doctests; all 62 controller groups also pass in
  release. [Content scheduling](archive/plans/dtd-content-controller.md) and the
  [Writer protocol](archive/plans/dtd-writer-protocol.md) have reviewed outlines.
  Public activation, attribute/content integration and its full validation
  gates remain open.
  The subsequent [tag and Writer review](archive/review-dtd-tags-writer.md) records atomic
  default/namespace preparation, local token/reference foundations and the
  private Writer protocol: the coordinator/ledger follow-up passes 942 ordinary
  tests, all 504 library tests in release
  and 13 doctests pass. Review corrected capacity and diagnostic accounting and
  a release-only publication defect. A measured staging regression led to
  restoring the ordinary one-pass tag path; retained repeats support removal
  of the large cost without establishing zero regression. Generated content
  scheduling, public activation and fresh DTD fuzz/conformance evidence remain
  open.
  Final Writer review also corrected known event/item quota checks occurring
  after encoding; an independent regression and three runtime faults verify the
  preflight order. Eight independent Text/reference boundary groups pass,
  including actual UTF-16 decoding and exact physical spans. The private
  [generated-event preparation path](archive/plans/dtd-parser-event-preparation.md)
  now owns replacement leaf/Start/End fields, preserves replacement EOL phase
  and copies End names before scope unwind. Eleven independent groups include
  nonempty generated Start retention and one-short copy failure atomicity.
  Source-bound coordination now includes its aggregate Input/Local/reference
  scratch reservation and nine independent debug/release groups. The same work
  ledger admits external catalog/frame deltas before publication. Independent
  review found and corrected a resumed-position arithmetic panic; exact typed,
  sticky overflow controls pass. The subsequent private content controller now
  moves that same ledger, catalog, frame stack, parser and ownership Budget
  through physical and nested generated content. It drains empty and nested
  entities finitely, preserves exact replacement origins, checks frame guards
  before generated End publication and keeps callback cancellation terminal.
  Nine author groups and 21 independent groups pass in debug and release; the
  independent suite catches removed complete-token work accounting. This is a
  reviewed private composition checkpoint, not frontend activation.
  That [private streaming-session outline](archive/plans/dtd-streaming-session.md) now
  has a reviewed callback implementation on `integrate/dtd-prototype`. It moves
  the same Decoder/parser into one fallibly reserved, out-of-line session and
  keeps one event ceiling, ownership Budget and standalone value through the
  declaration/content handoff. Eighteen author and independent groups pass in
  debug and release; deliberate production-route allocation, standalone and
  callback-panic faults are caught. The next private checkpoint carries actual
  UTF-16LE/BE encoding and distinct decoded/physical cursors through the same
  Decoder, declarations and generated body content. Its 37 combined streaming
  groups cover six transports, every byte split, non-BMP entities, origins,
  quotas and terminal failures. The subsequent private
  [lending protocol](archive/plans/dtd-lending-publication.md) separates lexical
  preparation, one-shot semantic publication and acknowledgement across ordinary
  XML, DTD declarations and generated body content. Physical loans retain source
  storage; entity scheduling and DTD End handoff occur only after acknowledgement.
  Callback delivery uses the same publication core and releases only on success.
  Twenty authored/blind lending groups pass, an E0499 probe proves a live physical
  loan blocks mutation, and all six required production mutations are caught.
  The combined lending freeze passes 1,031 tests with nine ignored, 593 release
  library tests and 13 doctests. The next reviewed private
  [buffered composition](archive/plans/dtd-buffered-lending.md) automatically
  acknowledges the prior loan, commits exact `BufRead` progress before lending,
  drains generated and held work before source I/O, and preserves exact terminal
  I/O, panic, quota, and origin behavior. Its combined tree passes 1,062 tests
  with nine ignored in debug and release, including 624 library tests and 31
  authored/blind buffered groups. Six holistic and eight author mutations are
  caught. A second E0499 probe proves the frontend's physical event loan blocks
  another mutable read. The subsequent private
  [stable-input Reader](archive/plans/dtd-stable-reader.md) removes the acknowledgement
  loan from an immutable UTF-8 slice while preserving exact physical borrows,
  generated ownership, one parser/catalog/frame state and truthful high-water
  positions. Its [conformance plan](archive/plans/dtd-stable-reader-conformance.md) and
  [review](archive/review-dtd-stable-reader.md) cover retained lifetimes, repeated equal
  source occurrences, standalone and counter continuity, held-End handoff,
  allocation/unwind atomicity and single-pass lazy attribute normalization.
  The final private tree passes 670 library tests in debug and release and 13
  doctests. Forty-one runtime mutations are killed and four invalid lifetime
  constructions are rejected by the compiler; holistic review has no open
  findings. The subsequent private
  [quota-facade review](archive/review-dtd-reader-quota.md) maps all 40 applicable
  settings through the Reader boundary with 94 focused groups and 15 killed
  production faults. The first [private fuzz-driver review](archive/review-dtd-reader-fuzz.md)
  adds bounded direct replay, four structured families, exact occurrence and
  provenance oracles, retained-lifetime checks and an ignored stdin entry. The
  subsequent [boundary-matrix review](archive/review-dtd-reader-fuzz-step2.md) adds
  canonical witnesses, literal truncation points, all 48 quota and precedence
  axes, lifecycle receipts, typed retained-event checks and pre-clone evidence
  caps. Two independent reviews detect every requested route, lifecycle,
  storage, origin, accounting, snapshot and QName regression. The integrated
  tree passes 1,240 tests with ten ignored in debug and release all-target runs,
  plus 13 doctests. The next [protocol-parser review](archive/review-dtd-reader-fuzz-protocol.md)
  freezes the bounded `libtest-exact-v1` grammar, structured diagnostics and
  pinned real-libtest output with two independent PASS verdicts. The subsequent
  [Cargo selector review](archive/review-dtd-reader-fuzz-artifact-selector.md) adds a
  bounded full JSONL parser and exact current-package library-test selection,
  including linear raw-Unicode handling. The following
  [artifact-resolver review](archive/review-dtd-reader-fuzz-artifact-resolver.md) adds
  lexical and component-wise filesystem containment, same-handle identity and
  post-capture revalidation while preserving direct-target behavior. The
  [artifact-lifecycle review](archive/review-dtd-reader-fuzz-artifact-lifecycle.md)
  adds immutable metadata/content rechecks, bounded same-handle hashing and
  temporal drift classification for later campaign gates. Cargo process/profile
  integration, measured coverage and performance gates remain open. Public DTD
  events/errors, UTF-16 ownership, DOM storage and activation also remain
  explicit follow-ups.
- [x] Extend the independent construction fuzz model with twelve literal
  binding/name-limit precedence cases, observable UTF-8/UTF-16 surfaces, actual
  leaf-dispatch assertions and same-editor recovery. The
  [fresh validation](archive/validation-construction-binding.md) closes seven missed
  regions/lines without production changes: 6,885/7,331 regions, 4,756/4,923
  lines and unchanged 909/962 instrumented branch outcomes. The 100% target
  remains open; no exclusions or production hooks were added.
- [x] Review the initial public API and resource policy before expanding to buffered/push streaming; final API stabilization is a separate P1 gate.
- [x] Share semantic state between borrowing slices and bounded push input; retain only open-element names and live namespace scopes.
- [x] Add callback push parsing and a `Read` adapter with explicit final EOF, sticky cancellation/errors, I/O failure preservation, and chunk-independent event values/spans.
- [x] Add a bounded UTF-8 event writer sharing grammar and namespace validation, with explicit finish, escaping, prospective XML offsets, and terminal partial-I/O failures.
- [x] Add fresh-writer complete borrowed/owned DOM serialization with iterative
  traversal, UTF-8 declaration adaptation, final flush, strict semantic
  round trips, independent quotas, sink faults, and fuzz coverage.
- [ ] Add namespace repair, detached-subtree output, formatting, save options,
  and UTF-16 output with separate behavior contracts and regression coverage.
- [x] Add immutable owned document storage with pool-backed views, shared normalization/storage accounting, and input-independent lifetimes.
- [x] Add checked owned-document handles and bounded atomic leaf/existing
  ordinary-attribute value edits, with persistent provenance, cumulative edit
  capacity accounting and an independent operation model.
- [x] Add fresh owned leaf insertion and permanent subtree removal with stable
  checked handles, persistent edit quotas, iterative deletion and an independent
  operation model. Slots and retained string storage are not reclaimed or reused.
- [x] Add allocation-free borrowed and owned `text_chunks()` views in document
  order, with edited-tree and encoded-input checks and pinned external-oracle
  comparisons in the [validation report](archive/validation-text-chunks.md).
- [x] Implement bounded [rooted construction](archive/plans/owned-create-slice.md) and
  fresh element append with initial ordered attributes/declarations, typed
  diagnostics and persistent quotas. The [validation](archive/validation-owned-create.md)
  records independent tests, failpoints, the pinned oracle, source-complete fuzz
  coverage and Rust performance baselines. Existing-element attribute
  insertion/removal still needs an accepted membership design.
- [x] Extend independent construction validation, namespace and resource-limit
  catalogs; fix actual replay dispatch for sparse selectors; check complete
  topology against returned handles. [Fresh fuzz evidence](archive/validation-construction-fuzz-gaps.md)
  reaches 6,810/7,260 regions and 919/970 branch outcomes. Remaining allocation,
  overflow and defensive paths stay in the denominator; 100% remains open.
- [ ] Add general element/attribute and sibling insertion, detach/move/copy,
  cross-document import/adoption, namespace repair, reclamation/compaction and
  any slot-reuse policy with explicit cycle, ownership and text-merging contracts.
- [x] Add incremental UTF-16LE/BE input to push parsing and owned trees, with declaration agreement, raw/decoded ceilings and original byte spans; UTF-16 output remains pending.
- [x] Add bounded buffered pull input with lending events, exact source consumption,
  terminal I/O contracts, retained-state size series, and time-to-first-event
  benchmarks. The [matched baseline](archive/validation-parse-baseline.md) includes
  buffered throughput; comparative time-to-first-event latency and memory remain
  pending.

The reference release is pinned and 4,056 header identifiers are indexed. The
[configured inventory audit](archive/validation/libxml2-api-audit-21504.md) checks the
built 2.15.4 static oracle's public declarations and symbol visibility. Other
platform/configuration and shared-library checks, plus per-operation Rust
mappings, remain open. The crate has no C ABI exports.

Later rows receive their own detailed burndown when their dependencies are understood. Avoid building a generic engine for schemas, XPath, and HTML before implementing their actual semantics.

The [streaming contract](streaming.md) describes the implemented P1 subset. Push
and buffered pull parsing share one resumable engine and bounded token buffer;
neither accumulates the input. Additional encodings remain pending. The
[owned DOM contract](owned-dom.md) describes the implemented independent storage
mode; retained-memory and comparative performance evidence remain open.
Its [value-mutation validation](archive/validation-owned-mutation.md) and
[leaf-structure validation](archive/validation-owned-structure.md) record the implemented
editing slices and their evidence gaps; richer mutation remains open.

The [writer contract](writer.md) describes event and complete-document output.
Whole trees retain their explicit namespace declarations and are traversed
without recursion; document calls validate closure and flush before returning.
This remains a subset of libxml2's save/writer surfaces. The
[matched structural benchmark](archive/validation-owned-structure-matched.md) includes
counting serialization after edits with explicit buffering and topology
differences; a broader isolated serialization comparison remains pending. The
[complete-document validation report](archive/validation-dom-serialization.md) records
tests, mutation review, raw coverage, exploratory oracle, and benchmark-runner
evidence for this slice.

## Performance decisions

“Faster than libxml2 across the board” is the optimization target. It needs a workload matrix before it can be evaluated: tiny documents, large flat documents, deep nesting, attributes, namespaces, entity-heavy text, non-ASCII names, mixed content, malformed input, and multiple input chunk sizes. Include full DOM parse/destruction, streaming consume/discard, owned-input cost, time to first event, throughput, allocations, retained memory, and peak resident memory.

Compare release builds on the same machine with matched options, validation, limits, encoding, and event work. Record compiler, CPU, allocator, input hash, warm/cold policy, repetitions, median and spread. Parsing a borrowed input is different from copying and decoding an owned document; report both. A faster tokenizer is not a full DOM result. Run performance work in isolated experiments after correctness and security gates pass. Reject optimizations that weaken validation or turn valid input into an undocumented subset.

Use `memchr` for delimiter search when profiles justify it, borrow unchanged strings, and allocate normalized strings only when necessary. Favor contiguous arena storage and explicit stacks. Choose interning, extra indices, and compact node layouts based on measured workloads; each can trade parse time for memory or later query speed. Track adversarial scaling as well as typical throughput.

The [UTF-16 ASCII batching screen](archive/validation-utf16-ascii-batches.md) records
large long-token gains together with a repeated UTF-8 tiny-element regression.
That prototype remains off main; the preliminary samples do not establish an
accepted optimization or a libxml2 performance result.

Benchmark preflight found that [xmllint batch timing](https://github.com/GNOME/libxml2/blob/v2.15.4/xmllint.c)
excludes startup and memory mapping but reuses its DOM parser context. Its
streaming loop reads to EOF without fetching every name, value and attribute;
the [memory reader](https://github.com/GNOME/libxml2/blob/v2.15.4/xmlreader.c)
also copies the whole input. These differ from yaxml's current fresh-context,
borrowing DOM and bounded push measurements. Internal CLI timing is useful only
with those lifecycle and workload differences stated. The
[matched parsing baseline](archive/validation-parse-baseline.md) now uses separately
built executables, shared fixture bytes, calibrated batches and alternating
trials. Its phase data exposes parse deficits even where cheap Rust destruction
produces a combined DOM lifecycle win. Start with profiling long-text validation
and normalization scans, then per-element allocation and scoped namespace work;
preserve exact validation, error precedence and resource contracts. The broader
matrix remains required before claiming an engine-level performance advantage.

## Toolchain and allocation gate

Verify the user's Rust 1.100 allocator premise against the official issue, compiler availability, and a minimal compile probe before setting the minimum supported Rust version or making the public API generic over allocators. Standard-library allocator support, collection support, and the ability to implement a custom allocator without unsafe are separate questions. Keep the default API usable with the verified compiler, and document experimental configurations honestly. The requested reference is the [allocator API tracking issue](https://github.com/rust-lang/rust/issues/32838).

Document ownership and arena boundaries now; defer a pervasive generic allocator parameter until the toolchain gate passes and a benchmark demonstrates its value. Allocation failure and memory accounting still require audits regardless of allocator selection. An arena reduces allocation overhead; it does not bound expanded data by itself.

Status: probed on `rustc 1.100.0-beta.7`. The opt-in `allocator_api` feature puts owned-document nodes and attributes in a caller allocator without making the reader generic; the string pool stays global because `String` has no allocator parameter. Findings, boundary and remaining global allocations: [allocator strategy](design.md#allocator-strategy). Drop the feature gate once the MSRV reaches 1.100.

## References and review

Conformance follows [XML 1.0 Fifth Edition](https://www.w3.org/TR/2008/REC-xml-20081126/) and [Namespaces in XML](https://www.w3.org/TR/REC-xml-names/). UTF-16 input closes one gap; DTDs and the full conformance suites remain pending. The [libxml2 API reference](https://gnome.pages.gitlab.gnome.org/libxml2/) defines the comparison surface; version and build features must accompany every compatibility claim. The [security plan](security.md) defines the evidence needed for fuzzing and hostile-input readiness.

Delegated work is reviewed before it reaches local main. Review the actual diff, behavioral tests, error paths, complexity, dependencies, unsafe prohibition, and public API impact. Integrate small coherent changes; update the compatibility and coverage inventories in the same change. A passing test suite establishes only its tested scope, so milestone notes must state the remaining gaps.

## Remaining gaps (2026-10-19)

Audit of every feature row in [compatibility.md](compatibility.md) against `src/`, `tests/` and the module docs. Size: S under about a week, M a few weeks, L a quarter or more. Ordered by migration value first, then security, then bindings; items 1-7 are mostly API surface over data the parsers already hold.

1. Structured error record and handler (`xmlError`: domain, code, level, line, column, message; `xmlCtxtSetErrorHandler`, `xmlGetLastError`) - M - migration (every libxml2 caller reads it) and bindings (exception mapping); today `Error` is `{ kind, offset }`.
2. Line and column metadata (`xmlGetLineNo`, `xmlSetLineNo`, `XML_PARSE_BIG_LINES`, reader line/column getters, error line/column) - S - migration; offsets exist on nodes and errors, so this is an offset-to-line index.
3. DOM accessors (`xmlGetProp`, `xmlGetNsProp`, `xmlHasProp`, allocating `xmlNodeGetContent`/`xmlNodeListGetString`, `xmlGetNodePath`, previous sibling, `xmlNodeGetBase`/`xmlNodeSetBase`, `xmlNodeGetLang`, `xmlNodeGetSpacePreserve`, document URL) - S - migration and bindings; base logic already exists in `TextReader`, `xinclude` and `catalog` and should be shared.
4. Namespace API (`xmlSearchNs`, `xmlSearchNsByHref`, `xmlGetNsList`, `xmlNewNs`, `xmlSetNs`, `xmlReconciliateNs`, `xmlDOMWrapReconcileNamespaces`) - M - migration, and required before safe move/copy of subtrees.
5. Mutable DOM core (`xmlSetProp`, `xmlUnsetProp`, `xmlAddChild`, `xmlAddNextSibling`, `xmlAddPrevSibling`, `xmlUnlinkNode`, `xmlReplaceNode`, text merging, detached ownership, slot reuse/compaction) - L - migration and bindings (editing is the main reason to keep a DOM).
6. Copy, import and adopt across documents (`xmlCopyNode`, `xmlDocCopyNode`, `xmlCopyDoc`, `xmlDOMWrapAdoptNode`, `xmlDOMWrapCloneNode`) - L, after 4 and 5 - bindings and migration.
7. Context reuse/reset and capability reporting (`xmlCtxtReset`, `xmlCtxtResetPush`, `xmlCtxtSetOptions`, `xmlHasFeature`, `xmlCheckVersion`, version constants) - S - bindings (probe modules, amortize setup); no hook exists today.
8. Public DTD and entity model (`xmlGetDocEntity`, `xmlGetPredefinedEntity`, `xmlGetDtdElementDesc`, `xmlGetDtdAttrDesc`, `xmlGetDtdNotationDesc`, `xmlEncodeEntities*`) - M - migration; the parsed declarations are held privately in `valid::Dtd`.
9. Caller-supplied character converters (`xmlCtxtSetCharEncConvImpl`, `xmlRegisterCharEncodingHandler`, `xmlFindCharEncodingHandler`) - M - migration for Shift_JIS, EUC-JP, ISO-2022-JP, GBK and other iconv-only labels without bundling tables, keeping the core dependency-free.
10. Migration; UTF-16 and single-byte input on the borrowed `Reader`/`Document` (44 XMLTS gaps) - M.
11. `RECOVER` and `NOBLANKS` on `PushParser`, `BufferedReader` and `TextReader` (`xmlParseChunk` with `XML_PARSE_RECOVER`) - M - migration (lxml-style recover over streams).
12. DTD validation while streaming (`xmlTextReaderSetParserProp(VALIDATE)`, `xmlTextReaderPreserve`, `ReadAttributeValue`, `Close`) - M - migration for `xmlTextReader` users.
13. Fragment and in-context parsing (`xmlCtxtParseContent`, `xmlParseInNodeContext`, `xmlParseBalancedChunkMemory`) - M, needs 4 and 5 - migration; removes a reason for callers to string-splice XML.
14. Output sinks and writer API (`xmlSaveToIO`/`xmlSaveToFd` over `Write`, `xmlSaveTree` level, `xmlSaveSetEscape`, the `xmlTextWriter*` family, HTML subtree as HTML) - M - migration and bindings (streaming output without a `Vec`).
15. SAX2 handler table (`xmlSAXHandler`, `xmlSAX2*`, user-data callbacks over `PushParser`) - L - migration for SAX consumers and bindings (lxml target parser); events cover the data, not the ABI shape.
16. Node-count and wall-clock/work budgets for parsing, sustained fuzz campaigns and the remaining coverage denominator - M - security; today input, depth, events and owned-byte limits bound parsing but not node count or CPU time.
17. XML 1.1 and the namespace-unaware profile (611 unscored XMLTS cases per surface, 9 namespace-off cases) - L - migration, low (rare in practice).
18. HTML gaps from [html.md](html.md) (multi-byte converters, SAX callbacks, `HTML_PARSE_HTML5`, `htmlCtxtParseContent`, line numbers) - M - migration for HTML users.
19. Schema and catalog remainders (`xmlRelaxNGDump`, XSD stream `ID`/`ENTITY` verdicts, two XSD counter quirks, catalog editing/dumping and the catalog PI, `CATALOG_PI` flag) - S each - migration, low.
20. Fill the `status` column of `docs/libxml2-api.tsv` (4,056 rows `unmapped`) from the feature rows, and add a per-symbol Rust mapping - M, mostly scripting - process: no inventory row reflects what exists.
21. Language bindings and any C ABI (`yaxml-capi`, Python, Node, Ruby, Go) - L - bindings; blocked on owner sign-off for unsafe adapters per [bindings.md](bindings.md), and easier once 1, 3 and 7 land.
22. Public utility helpers (`xmlunicode.h` category/block queries, `xmlstring.h`, `chvalid.h`, `debugXML.h` dumps) - S - migration, low; `std` covers dictionaries, hashes and lists.
