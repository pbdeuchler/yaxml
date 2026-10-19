# Security and fuzzing plan

This is a development policy, not a claim that yaxml is ready for hostile production input. Safe Rust prevents classes of memory errors in this crate; it does not prevent excessive allocation, slow algorithms, stack exhaustion, incorrect XML interpretation, or mistakes in downstream bindings. Fuzzing supplies evidence about tested executions, not proof that vulnerabilities are absent.

## Trust boundaries

The Rust core forbids unsafe code and has one dependency, `memchr`. Keep the same restriction for core tests, examples, and fuzz drivers; only the standalone `fuzz/` cargo-fuzz package adds `libfuzzer-sys` and its generated unsafe entry point. Review the dependency graph and enabled features when updating the lockfile. The prohibition applies to yaxml's source; dependencies and the standard library have their own implementation and audit boundaries.

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
| Structure | Element depth, attributes per element and event count; parsed node creation is bounded by events; fresh owned nodes and attributes have persistent counts; construction and subtree removal are iterative | Retained DOM bytes, existing-element namespace edits, general insertion and cycle checks, moves/import/adoption, detached ownership and slot reclamation/reuse |
| Entities | Entity nesting, reference count, expanded bytes, expansion work and cycle detection, shared by internal and external entities | Validation-time content models |
| External resources | No I/O without a caller `Resolver`; `max_external_loads` requests and `max_external_bytes` cumulative response bytes per document, checked before decoding; nesting/expansion share the entity budgets | Redirects and cancellation belong to the caller's resolver |
| Query/validation | Unsupported initially | Expression size, evaluation steps, intermediate results, schema state and recursion |
| Output | Encoded event and total byte ceilings, depth/attributes/events/owned strings; validation before sink I/O; iterative complete-tree serialization; partial positions and terminal errors | Namespace repair, subtree output, transcoding, canonicalization and their additional budgets |

Input size alone does not bound all work. Many attributes can cause quadratic duplicate-name checks; deeply nested namespace rebinding can cause repeated scans. Benchmark adversarial size series and record the algorithmic bound for each such path. CPU budgets should count deterministic work where possible; wall-clock deadlines belong in the embedding application or an explicit cancellation interface.

The first implementation uses a scoped namespace index and interned namespace
identities for duplicate-attribute checks. This avoids rehashing a long borrowed
URI for every attribute. Regression tests preserve aliases and scope; the
[validation report](validation.md) records the adversarial probe and coverage gaps.

Use fallible reservation for attacker-sized collection growth. Audit implicit allocations, string clones, formatting, and collection constructors too. Returning an allocation error from some paths does not establish that every out-of-memory condition is recoverable. Record remaining infallible allocations before claiming graceful allocation failure. Owned editing has a safe thread-local test seam that simulates returned failures at its node, attribute, pool and construction-scratch reservation boundaries; it proves those error paths and their publication atomicity, not allocator internals. Exercise allocator failure separately once the selected toolchain and safe allocator interfaces support it; libxml2's fuzzers deliberately inject allocation failures, which is a useful testing precedent. [libxml2 fuzzing instructions](https://raw.githubusercontent.com/GNOME/libxml2/master/fuzz/README.md).

## External resources and compatibility

No document-controlled filesystem or network access is implicit. External DTD subsets and entities load only through a `Resolver` the caller passes in; XInclude uses the same path (`ResourceKind::XInclude`, `XIncludeText`), with its loads, bytes and result size bounded across the whole call; future catalog and schema support must too. The core sends a `Request` with the resource kind, the system ID resolved against the declaring resource's base, the public ID and the remaining byte budget; the caller decides access policy. A resolver can return in-memory bytes without granting ambient filesystem access. Limits are cumulative across recursive loads, never per resource, and loaded text shares the document's entity expansion, reference and depth budgets, so external "billion laughs" stops at the same ceilings.

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
The [construction evidence](validation-owned-create.md) and
[subsequent fuzz-gap audit](validation-construction-fuzz-gaps.md) retain
unexecuted error paths in their fuzz-only denominators. Independent validation,
quota and namespace catalogs now enter through every bounded replay input;
complete execution coverage remains open.

## What “100% fuzzing coverage” means

The [complete XMLTS catalog run](validation-xml-conformance.md) is separate
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

The dependency-free driver backs a stdin/file replay example, deterministic smoke tests, and one cargo-fuzz (libFuzzer) target per input family; `fuzz/coverage.sh` reports per-target and merged library coverage. Random mutation without coverage feedback is a smoke test, not evidence of coverage-guided fuzzing. See [fuzz/README.md](../fuzz/README.md).

## Release gates

Every implementation milestone must have regression tests for accepted and rejected behavior, corpus replay, resource-limit tests, target-inventory updates, and a coverage artifact. Before a production release, require the raw fuzz-only function and region coverage reports, branch gaps, independent differential results, allocation audit, dependency audit, and adversarial complexity results. Any unreachable region needs a written justification or removal. A percentage below the stated goal must be reported as a gap, not rounded to completion.

Continuous fuzz infrastructure, sustained run budgets, private vulnerability reporting, supported release policy, and independent security review remain release prerequisites to establish. No elapsed fuzzing duration alone proves security. Security fixes need a minimized regression, affected-version assessment, and a release note that describes the actual impact.
