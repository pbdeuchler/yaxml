# Design direction

This is a working outline, not a frozen architecture. The first implementation
is a strict XML 1.0 UTF-8 reader and immutable DOM. The feature inventory defines
the larger compatibility target; completing this first slice does not establish
libxml2 replacement status.

## Constraints and compatibility

The library forbids unsafe code and uses Rust's standard library plus `memchr`.
No feature may quietly introduce another library dependency. `memchr` and the
standard library can contain their own unsafe implementation; the enforceable
crate guarantee is `#![forbid(unsafe_code)]`. Reimplementing their internals would
not improve that boundary.

Compatibility has three separate deliverables:

1. **Behavior:** accepted documents, normalized values, namespaces, tree shape,
   errors, serialization, and requested processing features.
2. **Migration APIs:** Rust entry points and later language adapters that let
   callers perform their existing operations with explicit ownership.
3. **C source/ABI compatibility:** symbol names, exposed struct layouts, pointer
   identity, callbacks, ownership, and allocator hooks expected by existing C
   callers.

The first two fit this core. Full C ABI compatibility with libxml2's mutable,
public pointer graph is a separate problem. Its public [tree API](https://gnome.pages.gitlab.gnome.org/libxml2/html/tree_8h.html)
exposes nodes, attributes, namespaces, and document ownership. An index arena
cannot impersonate that layout. Receiving and dereferencing arbitrary C pointers
also requires an unsafe boundary. We should report this incompatibility before
claiming a binary drop-in replacement. No unsafe binding adapter is authorized
by this design; keep the pure Rust core usable independently.

Use XML specifications as the strict-mode authority. Use a versioned libxml2
build as the behavioral reference for compatibility options, tree conventions,
and recovery. A mismatch is a triage result, not permission to imitate an oracle
bug. Record differences with the input, options, reference revision, and reason.

## Lessons from the references

These are design choices informed by the references, not claims that their
complete behavior has been reproduced.

| Reference | Structure worth adopting | Tradeoff to address |
| --- | --- | --- |
| [libxml2 parser API](https://gnome.pages.gitlab.gnome.org/libxml2/html/parser_8h.html) | Parser contexts, DOM/SAX/pull/push entry points, explicit resource loaders, amplification limits. | Legacy globals and permissive compatibility options complicate isolation. Keep policy and diagnostics per operation; require explicit access to external resources. |
| [quick-xml 0.42 reader](https://docs.rs/quick-xml/0.42.0/quick_xml/reader/struct.Reader.html) | Events borrowed from slices, caller-reused buffers for I/O, ownership conversion when callers retain data. | A low-level event stream is not automatically a validating, namespace-aware XML processor. Compare equivalent processing work in benchmarks. |
| [quick-xml 0.42 configuration source](https://docs.rs/quick-xml/0.42.0/src/quick_xml/reader/mod.rs.html) | Explicit parser state and configurable behavior. | Comment checking defaults off; per-event text trimming has mixed-content caveats. Our strict mode validates comments and preserves significant whitespace. Avoid option combinations that silently weaken its contract. |
| [roxmltree 0.20 source](https://github.com/RazrFalcon/roxmltree/blob/v0.20.0/src/lib.rs) | Compact index-based tree links, borrowed input text, allocation only when normalization requires it, iterator-based navigation. | Read-only storage simplifies lifetimes and traversal. Mutation, owned input, and stable foreign handles need deliberate later designs rather than promises of zero cost. |
| [xmloxide 0.5 module map](https://docs.rs/xmloxide/0.5.0/xmloxide/) | Separate parsing, tree, reader, SAX, query, validation, serialization, encoding, and inclusion modules; document-scoped node IDs. | Broad feature coverage expands the verification surface. Deliver independently tested subsystems and preserve common parser semantics across entry points. |
| [xmloxide FFI](https://docs.rs/xmloxide/0.5.0/xmloxide/ffi/index.html) | Explicit document/string ownership and a separate foreign interface. | Its raw-pointer interface is explicitly unsafe and uses a different symbol prefix. A pure Rust replacement and a libxml2 ABI replacement are distinct claims. |

The historical roxmltree benchmark table explicitly cautions that different
feature sets limit comparisons. Treat published results as hypotheses for our
own workload matrix, not as evidence of yaxml performance. Its [README](https://github.com/RazrFalcon/roxmltree/tree/v0.20.0)
also explains the read-only choice and input-borrowing strategy.

## One parser with several consumers

Start with concrete internal functions and a small state machine:

```text
bytes -> encoding boundary -> XML scanner + document state + namespaces
                                                |
                                      validated logical events
                                   /            |             \
                               pull          DOM builder      SAX
```

The scanner finds delimiters with safe slice operations and `memchr`. XML name,
character, reference, declaration, and namespace rules remain explicit. An ASCII
fast path must have the same semantics as the Unicode path. Share those rules
between text and attribute processing where their normalization rules agree;
keep their differences visible.

The parser owns document phase, element stack, namespace scope, and limits.
Parsing is iterative; input depth must not become Rust call-stack depth. Every
step either consumes input, emits an event, requests more input, or returns an
error. Errors terminate strict parsing; repeated calls must have a documented
terminal result. EOF validates the final state rather than merely exhausting
bytes.

The pull API comes first. Build the DOM directly from its validated events. This
makes streaming and DOM normalization agree and gives us a differential test
inside the crate. Avoid constructing a DOM to implement SAX or a reader. If
temporary event allocation becomes a measured DOM bottleneck, add a concrete
internal event-consumer path that shares the same state transitions. Do not
create a public generic parser framework in anticipation of that result.

`Reader` over a complete byte slice emits borrowed events while retaining the
supplied input. `PushParser` implements incremental `feed`/`finish` with a separate
bounded lexical scanner. It retains a raw token until its delimiter arrives,
including text until the next `<` or final EOF, so event boundaries and token
limits are independent of chunking. The lexer carries quote/delimiter state
instead of rescanning an accumulated prefix. Safe `memchr` searches copy ordinary
byte runs in bulk. A four-byte preamble handles split encoding signatures.

Both frontends use `ParserState` for names, declarations, normalization, document
phase, namespaces, and budgets. A narrow retention callback borrows slice strings
or fallibly copies streaming structural strings. Namespace IDs have scoped
reference counts; completed scopes release inactive URI entries. Events never
borrow owned parser state, permitting scope removal before event delivery.

Push callbacks borrow events for their call and receive raw source ranges. A
callback break is terminal cancellation. `read_from` is a thin buffered `Read`
driver with explicit I/O failures. `BufferedReader` provides lending pull events
over bounded `BufRead` input using the same incremental engine. Both preserve
terminal I/O failures rather than allowing ambiguous resumption after an
external error. Async runtimes remain caller concerns. Split-point tests cover
UTF-8 sequences, CRLF, references, quotes, comments, and CDATA terminators.

Whole-input slice validation and incremental token validation can choose different
first errors when several defects occur. Streaming can deliver a valid prefix
before a later failure. Grammar acceptance and logical event/spans agree for
successful documents; resource comparisons account for streaming's additional
retained string copies. No frontend silently buffers the entire stream to imitate
the slice constructor's error precedence.

## Values, namespaces, and diagnostics

The existing public vocabulary includes `Reader`, `PushParser`, `Writer`,
`Event`, `Element`, `Attribute`, `Declaration`, `Name`, `Limits`, `WriterLimits`,
`Document`, `Node`, `NodeId`, `NodeKind`, `OwnedDocument`, `OwnedNode`, `OwnedNodeId`,
`Editor`, `EditLimits`, `EditError`, `EditErrorKind`, and `EditLimit`.
Retain these names while behavior stabilizes. New parser policy should extend
explicit configuration, with strict XML as the default; avoid a bag of
independent switches that permits contradictory contracts.

`Name` must distinguish lexical qualified name from expanded name (namespace
URI plus local name). End tags match the lexical start name. Attributes must be
unique by expanded name after namespace declarations are processed. The default
namespace applies to unprefixed elements, not unprefixed attributes. Preserve
prefixes and declaration order when serialization or compatibility requires
them; namespace identity does not depend on prefix spelling.

Borrow unchanged UTF-8 text. Allocate on the first normalization change and keep
source offsets separate from normalized values. Character references preserve
their specified character; source whitespace normalization must not rewrite
them as if they were literal source whitespace. Do not trim mixed content as a
parser shortcut. DTD-driven attribute normalization is a later semantic layer.

Keep `ErrorKind` and a zero-based byte offset machine-readable. Add source IDs
when external inputs arrive; add line/column mapping without replacing offsets.
Distinguish malformed XML, unsupported features, exhausted resource budgets,
allocation failure, and underlying I/O errors. Future recovery should return a
tree plus diagnostics and a clear incomplete/recovered status. It must never
turn failure into an apparently valid strict document.

## Output validation

The first `Writer` encodes one event into a reusable bounded scratch token,
validates its XML characters and raw delimiters, then submits it to the same
semantic state machine as the readers. Supplied public name and declaration
metadata must agree with the parsed token before sink I/O begins. This prevents
caller-created events from bypassing grammar or namespace checks while keeping
one document-state implementation. It costs an extra scan and temporary event
allocation; measure those costs before replacing the path with direct semantic
operations.

The sink is an explicit `Write` capability. Output limits count encoded bytes;
confirmed partial writes remain visible, and a failed or panicking operation
poisons later calls. Completion validates closure and flushes without generating
missing end tags. Namespace repair, pretty printing and raw insertion each need
a separate contract. Formatting can change mixed content, and repair can change
prefix spelling, so neither belongs in the strict default implicitly. See the
[writer contract](writer.md) for the current API and normalization behavior.

## DOM ownership and mutation

Use document-owned contiguous node storage with indices for parent/child/sibling
relationships. Keep attributes contiguous where practical. This avoids one heap
allocation per node and permits safe navigation after storage grows. A `Node`
view borrows its document; no public reference points into a `Vec` across a
mutation. Keep storage representation private so measurements can change it.

`Document` borrows input and remains an immutable performance baseline.
`OwnedDocument` owns a logical UTF-8 pool and stores ranges rather than references
into itself. Names and attributes resolve to borrowed views at access time;
prefix/local parts share one stored qualified spelling. Original source spans
are separate from pool ranges. This supports long-lived application state and
future bindings without self-references or changing the borrowing API.

Both forms consume the same validated parser events. Borrowing records retain
their compact immutable preorder layout; owned records separately carry the
links and live subtree counts required for mutation. Owned pool growth charges
full new capacities to the parser's cumulative normalization budget before
reservation. Namespace values are copied into owned storage; repeated-URI
storage and other layout optimizations need adversarial size-series evidence.
The [owned contract](owned-dom.md) records the view API, limits and remaining
storage work.

The borrowing tree's `NodeId` remains document-local; checked lookup rejects
out-of-range slots but does not detect foreign documents. Owned trees use
`OwnedNodeId` with a private non-recycled document identity and slot, rejecting
foreign handles and preserving identity when a document moves. Parsed owned
source ranges are optional persistent provenance, independent of later logical
content. Removed owned slots are permanent tombstones and are never reused, so
an old handle cannot identify a new node. Neither type promises stable C
addresses or replaces a future binding registry.

The first mutation slices replace existing leaf and ordinary-attribute values,
append fresh Text/CDATA/comment/PI leaves and permanently remove non-root
subtrees through an exclusive `Editor`. They validate before reservation and
publish only after every fallible step succeeds. Original source locations
remain provenance; they do not describe rewritten bytes. Editing has cumulative
capacity, created-slot and created-attribute counters that persist across editor sessions, with
parsed storage as the baseline and no refunds on removal.

Rooted creation and fresh element append accept borrowed name/attribute specs.
They gather explicit declarations before resolving names, preserve exact prefix
requests and bound automatic scope lookup by deterministic work units. Planning
uses fallible input/work-bounded scratch, then reserves persistent arenas before
publishing the element. Each new element receives one contiguous attribute
range; existing attribute read paths stay shared with parsed trees. This avoids
choosing a general attribute-membership representation prematurely.

Owned records carry reciprocal parent/child/sibling links and cached live
subtree sizes. Parsed and value-only documents keep contiguous-preorder
traversal; the first structural success switches permanently to linked traversal.
Removal clears each record to a tombstone iteratively while retaining its slot
identity. The pool and attribute arena remain append-only, so removal is logical
deletion rather than memory reclamation or secret-data erasure. Existing-element
attribute insertion/removal, sibling insertion, detach/move/copy, cross-document
import/adoption, namespace repair, compaction and slot reuse remain future
contracts. Those operations must preserve acyclicity, single ownership,
namespace well-formedness and stale-handle rejection. See the
[owned contract](owned-dom.md) for the implemented lexical, placement, quota and
error-order rules.

## Allocator strategy

As checked on 2026-10-15, [Rust PR #156882](https://github.com/rust-lang/rust/pull/156882)
merged on September 23. It stabilizes an MVP: `Allocator`, `Global`/`System`, and
minimal allocator-aware `Vec`/`Box` APIs. Other functionality remains under
[`allocator_ext` #163177](https://github.com/rust-lang/rust/issues/163177).
The [original tracking issue](https://github.com/rust-lang/rust/issues/32838)
records expected delivery in Rust 1.100, conditional on the release process.
Installed older compilers do not verify that release's behavior. A probe on
`rustc 1.100.0-beta.7 (2026-10-17)` compiles `Vec::new_in`, `try_reserve`
and `Box::new_in` with a generic `A: Allocator` under `forbid(unsafe_code)`.
Design the allocator API against 1.100 and gate it by MSRV until 1.100 is stable.

Use safe collection APIs with an allocator supplied by the application or the
standard library. `Allocator` remains an unsafe trait; yaxml cannot implement
its own bump allocator or allocation-counting wrapper under `forbid(unsafe_code)`.
Index arenas built from ordinary vectors require no custom allocator.

The first allocator milestone should cover DOM node and attribute vectors, then
parser stacks and normalized text. Borrow one allocator into several vectors to
avoid relying on `A: Clone` semantics. Prefer fallible reservation and report
failure before publishing partial state. Do not assume `String`, `Cow`, shared
strings, or every collection becomes allocator-aware with this stabilization.
Document exactly which allocations still use the global allocator.

Do not make the strict reader generic over an allocation framework merely to
claim allocator support. Add allocator parameters where storage is actually
allocated, with ordinary defaults. Validate the final public `parse_in` shape on
the target compiler before committing to it. A temporary feature gate on older
nightlies is experimental, not a claim of Rust 1.100 release validation.

## Security boundaries

Safe Rust removes a major class of memory errors in our code; it does not bound
CPU, memory, entity amplification, or external effects. Budgets belong in the
parser state and must be checked before allocating or performing expensive work.
All size arithmetic needs checked addition/multiplication. Limits must apply to
recovery and skip operations as well as normal parsing.

The first slice limits input bytes, token bytes, nesting, attributes, event
count, and cumulative requested string capacity for normalization and namespace
copies. DOM node/text totals, namespace work, query steps, schema states, expanded
entity bytes, external bytes, and inclusion depth are required as those features
arrive. Distinguish the input buffer supplied by the caller from extra memory
retained by the parser. A byte ceiling alone does not prevent quadratic work on
many attributes or namespace declarations; measure and bound those paths too.

No implicit network, filesystem, process environment, or global catalog lookup
belongs in parsing. A future resolver is an explicit per-operation capability
with a shared budget and cancellation policy. DTD/entity support can then be
added without changing the default trust boundary. Unsupported DTDs and encodings
must be rejected explicitly until implemented.

Every public byte-processing surface needs a fuzz target and a maintained corpus.
Use structural invariants, independent specifications, and versioned external
oracles alongside panic detection. Exercise valid and invalid input, limits,
terminal states, normalization, and future chunk boundaries. Track source-region
and branch coverage separately from API-target coverage. The goal is 100%
reachable crate coverage with reviewed exclusions; any reported percentage must
include the compiler, enabled features, corpus, execution budget, and denominator.
No coverage percentage proves absence of security defects.

## Performance evidence and bindings

Measure full DOM parsing, event consumption, actual bounded I/O streaming,
serialization, and destruction separately. Include tiny files, large text,
attribute-heavy documents, namespace-heavy documents, Unicode, normalization,
deep nesting, and adversarial failures. Record throughput, latency, allocations,
and peak memory. Match parser options and materialized outputs across libxml2
and yaxml. Fix the reference version, compiler settings, hardware, and corpus.
Beating libxml2 across workloads is the target; a geometric mean must not conceal
individual regressions. No claim is justified by an unvalidated microbenchmark.

Python, Ruby, JavaScript/TypeScript, and Go benefit from owned document handles,
batch traversal, stable error categories, bounded operations, and explicit
string lifetimes. Keep language runtime ownership and garbage collection out of
the core. Prefer bulk iteration or query results over one foreign call per byte
or attribute. Browser integration can use a future separately scoped WebAssembly
adapter, but no runtime dependency enters the core by accident.

Bindings may require unsafe code or ecosystem dependencies in a separate adapter
package. That requires an explicit scope decision later; it is not an exception
to today's constraints. Designing for adapters now means choosing portable
ownership and IDs, not implementing speculative wrappers.

## Order of work

Land strict UTF-8 parsing, namespace correctness, bounded failure, and DOM
traversal together with regression tests and fuzz entry points. Next establish
the conformance/differential harness and honest performance baseline. Callback
push input, the `Read` adapter, buffered lending pull input and strict UTF-8
event output now share that semantic engine. Owned storage builds independent
trees from its events; complete-tree serialization and bounded leaf/value edits
are implemented. UTF-16 input shares incremental decoding across owning and
streaming interfaces. Bounded rooted creation and fresh element append retain
contiguous attributes, and a finite feedback-guided campaign is available.
The next work includes broader construction fuzz coverage, sustained campaigns,
existing-element attribute membership, broader comparative performance evidence and
allocator support on the target compiler. HTML parsing and serialization
reuse owned storage through a separate libxml2-dialect tokenizer and tree
builder ([html.md](html.md)). DTDs, remaining encodings, XPath, validation,
canonicalization, inclusion and adapters each need their own feature burndown
and verification criteria. Recovery is an
explicit compatibility project with a versioned oracle, not a shortcut around
strict-parser failures.
