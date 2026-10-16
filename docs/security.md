# Security and fuzzing plan

This is a development policy, not a claim that yaxml is ready for hostile production input. Safe Rust prevents classes of memory errors in this crate; it does not prevent excessive allocation, slow algorithms, stack exhaustion, incorrect XML interpretation, or mistakes in downstream bindings. Fuzzing supplies evidence about tested executions, not proof that vulnerabilities are absent.

## Trust boundaries

The Rust core forbids unsafe code and has one dependency, `memchr`. Keep the same restriction for core tests, examples, and fuzz drivers. Review the dependency graph and enabled features when updating the lockfile. The prohibition applies to yaxml's source; dependencies and the standard library have their own implementation and audit boundaries.

Treat document bytes, parser options, caller-created tree data, resolver responses, and binding handles as independent input boundaries. Validate bytes and XML structure before creating trusted names and nodes. A writer must validate caller-created data even when the reader already validates parsed input. Constructors and tree mutations must preserve node relationships and namespace invariants. Do not add redundant validation inside every internal function.

Use iterative stacks for input-driven nesting, including parsing, traversal, serialization, entity processing, and destruction. Avoid one heap object whose recursive ownership follows each input element. Check additions, multiplications, offsets, and conversions before using them as sizes or indices. A parser error terminates ordinary event production; diagnostic recovery, if implemented, is a separate mode with an explicit contract. XML defines fatal-error handling and requires normal processing to stop after such an error. [XML 1.0, terminology](https://www.w3.org/TR/2008/REC-xml-20081126/#sec-terminology).

The parser rejects DTDs and unsupported encodings explicitly. It must not silently skip a construct and return a misleading success. Borrowing interfaces require UTF-8; owned and streaming input also accept UTF-16 under the [encoding contract](encoding.md). Decoded totals and token bytes have separate finite limits, malformed surrogates fail, and the decoder retains constant carry rather than a complete expanded source.

## Resource policy

Limits are per operation and checked before retaining more state or allocating a derived buffer. Their defaults are part of the public behavior and need boundary tests. A caller can choose different finite limits; a compatibility option must not quietly disable unrelated limits.

| Resource | Initial enforcement | Extension required before the related feature ships |
| --- | --- | --- |
| Input | Total source bytes | Aggregate resolved bytes, decoded bytes, and compressed input handled outside the core |
| Tokens | Raw token bytes | Decoded token bytes and transcoding carry buffer |
| Owned strings | Cumulative requested string capacity, including namespace copies, streaming names and full new owned-DOM pool capacities; one shared normalization/storage counter | Allocator-level byte accounting and failure injection; current budget is not a process memory quota |
| Structure | Element depth, attributes per element, event count | DOM nodes, retained DOM bytes, namespace declarations and bindings |
| Entities | Reject DTDs; recognize only predefined and numeric references | Entity nesting, reference count, expanded bytes, expansion work, and cycle detection |
| External resources | No implicit I/O | Request count, cumulative bytes, nesting, redirects, and caller cancellation |
| Query/validation | Unsupported initially | Expression size, evaluation steps, intermediate results, schema state and recursion |
| Output | Encoded event and total byte ceilings, depth/attributes/events/owned strings; validation before sink I/O; partial positions and terminal errors | Namespace repair, tree serialization, transcoding, canonicalization and their additional budgets |

Input size alone does not bound all work. Many attributes can cause quadratic duplicate-name checks; deeply nested namespace rebinding can cause repeated scans. Benchmark adversarial size series and record the algorithmic bound for each such path. CPU budgets should count deterministic work where possible; wall-clock deadlines belong in the embedding application or an explicit cancellation interface.

The first implementation uses a scoped namespace index and interned namespace
identities for duplicate-attribute checks. This avoids rehashing a long borrowed
URI for every attribute. Regression tests preserve aliases and scope; the
[validation report](validation.md) records the adversarial probe and coverage gaps.

Use fallible reservation for attacker-sized collection growth. Audit implicit allocations, string clones, formatting, and collection constructors too. Returning an allocation error from some paths does not establish that every out-of-memory condition is recoverable. Record remaining infallible allocations before claiming graceful allocation failure. Exercise allocator failure separately once the selected toolchain and safe allocator interfaces support it; libxml2's fuzzers deliberately inject allocation failures, which is a useful testing precedent. [libxml2 fuzzing instructions](https://raw.githubusercontent.com/GNOME/libxml2/master/fuzz/README.md).

## External resources and compatibility

No document-controlled filesystem or network access is implicit. Future DTD, XInclude, catalog, and schema support receives an explicit resolver from the caller. The core requests a resource with its kind, base URI, resolved identifier, and remaining budget; the caller decides access policy. A resolver can return in-memory bytes without granting ambient filesystem access. Apply cumulative limits across recursive loads, not a fresh budget per resource.

Keep resource policy distinct from syntax compatibility. libxml2 documents that entity substitution can load external entities, DTD validation can be vulnerable to complexity attacks, and its `NONET` option is not a general restriction on custom loaders. A security profile cannot be implemented by copying that single flag. [libxml2 parser options](https://gnome.pages.gitlab.gnome.org/libxml2/html/parser_8h.html#xmlParserOption).

Bindings require a separate design review. Opaque handles must reject stale and foreign-document identifiers; ownership and release must be explicit; strings need lengths and an encoding contract; panics must not cross a foreign boundary. Raw-pointer ABI glue cannot be promised under a whole-project unsafe prohibition. Resolve the binding mechanism and policy before implementing it, without weakening the core's prohibition by accident.

## What “100% fuzzing coverage” means

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
| Writer | Parsed-event round trips and generated caller-created operation sequences; explicit encoded ceilings; invalid values and state transitions; partial/Interrupted/invalid-count/erroring sinks; terminal failures perform no further I/O |
| Mutation, when implemented | Generated edits; reject cycles, stale handles and illegal namespace/ownership transitions; serialize and compare structure |
| Advanced features, when implemented | Separate grammar targets plus resolver, query, schema, entity, and cancellation operation sequences; assert budgets and deterministic results |

Differential testing runs a separately built, version-pinned libxml2 executable outside the Rust dependency graph. Match options and limits before comparing. Compare acceptance and a documented semantic projection of events/trees; attribute order, text event segmentation, diagnostics, and recovery require explicit rules. A disagreement is a triage item, not automatic proof that either implementation is correct. Check the W3C specification and add the minimized case to regressions. Namespace duplicate checks must compare expanded names, and unprefixed attributes have different namespace rules from elements. [Namespaces in XML](https://www.w3.org/TR/REC-xml-names/).

Use both arbitrary byte mutation and grammar-generated valid documents. Seed all lexical constructs, Unicode/name boundaries, CR/CRLF, references, malformed delimiters, namespace shadowing, duplicate attributes, truncation at every byte, and each limit at zero, below, equal, and above its boundary. Add deep/wide documents and many shared-prefix names to test complexity. Keep every minimized bug as a deterministic regression with its invariant, not just as an opaque corpus file.

Start with a dependency-free stdin/file replay driver and deterministic mutation smoke test. External AFL++ can launch the stdin driver, but efficient instrumentation and execution feedback need validation for the chosen Rust toolchain. Random mutation without coverage feedback is a smoke test, not evidence of coverage-guided fuzzing. Do not introduce `libfuzzer-sys`, a second Cargo dependency, an unsafe export, or a separate tooling crate without an explicit policy decision.

## Release gates

Every implementation milestone must have regression tests for accepted and rejected behavior, corpus replay, resource-limit tests, target-inventory updates, and a coverage artifact. Before a production release, require the raw fuzz-only function and region coverage reports, branch gaps, independent differential results, allocation audit, dependency audit, and adversarial complexity results. Any unreachable region needs a written justification or removal. A percentage below the stated goal must be reported as a gap, not rounded to completion.

Continuous fuzz infrastructure, sustained run budgets, private vulnerability reporting, supported release policy, and independent security review remain release prerequisites to establish. No elapsed fuzzing duration alone proves security. Security fixes need a minimized regression, affected-version assessment, and a release note that describes the actual impact.
