# Language binding boundary

Status: design outline. No binding package exists yet.

## Where unsafe lives

The `yaxml` crate forbids unsafe code and depends only on `memchr`. Any
foreign-function boundary needs unsafe code or a framework that contains it:
PyO3 for Python, magnus or a C ABI for Ruby, napi-rs or wasm-bindgen for
JavaScript, and cgo over a C ABI for Go. Bindings therefore live in **separate
crates**, such as `yaxml-capi`, `yaxml-py` and `yaxml-node`, that depend on
the safe core. The core's guarantees stay intact, and every unsafe line sits
in one audited adapter layer. **This needs explicit owner sign-off before any
adapter is written.**

A WebAssembly build of the safe core needs no unsafe code in yaxml. It copies
bytes in and results out, which makes it the lowest-risk JavaScript route.

## Shapes the core already provides

| Need | Core facility |
| --- | --- |
| Long-lived documents not tied to input lifetimes | `OwnedDocument` (also produced by `html::parse`) |
| Document memory charged to the host allocator | `OwnedDocument::parse_in`/`create_in` (`allocator_api` feature) |
| Stable node handles that reject stale or foreign use | `OwnedNodeId`: a private non-recycled document identity plus slot; removed slots are never reused |
| Pull streaming with no callbacks across the boundary | `TextReader` (xmlTextReader model), `BufferedReader` |
| Bulk results instead of one call per node | XPath `select_nodes` returns id vectors; serializers write whole documents to a `Write` |
| Errors that map onto host exceptions | `ErrorKind`, with a stable variant per libxml2 category, plus a byte offset |
| Hostile-input policy | `Limits` with finite defaults; no I/O unless a `Resolver` is supplied |

## Rules for core APIs that bindings will use

1. Handles are plain `Copy` values that can be validated, never references
   into storage. Every lookup goes through a checked accessor that returns
   `Option`.
2. A document is `Send`. Hosts with a global interpreter lock or a single
   thread confine a document to one thread. Shared read access across threads
   needs `&OwnedDocument` and no interior mutability.
3. Strings cross the boundary as UTF-8 byte slices. Each adapter converts to
   its host encoding; the core never assumes one.
4. Adapters map each `ErrorKind` variant to a stable integer code. New
   variants only ever add codes.
5. Resolvers and XPath extension functions are the only places the core calls
   host code. Both are explicit, `Send` and fallible. A host exception becomes
   an `io::Error` or XPath error rather than unwinding through Rust.
6. Allocator-aware constructors (`allocator_api`, Rust 1.100) let adapters
   charge document memory to the host runtime's allocator; see
   [Allocators](#allocators). That is a memory-accounting choice, not a
   correctness dependency.

## Allocators

With the `allocator_api` feature (Rust 1.100), an adapter can charge document
storage to the host runtime: `OwnedDocument::parse_in(input, options,
allocator)` and `create_in` return `OwnedDocument<A>`. The core cannot
implement `Allocator` because the trait is unsafe. The adapter crate already
contains the binding's unsafe code, so it supplies the implementation, for
example a wrapper over `PyMem_RawMalloc` or a counting quota allocator.

- Use one concrete `A` per adapter. `Arc<dyn Allocator + Send + Sync>` is an
  owned, `'static`, erased allocator, so wrapper types need no generics or
  lifetimes and documents stay `Send + Sync`. A document holds one clone per
  storage vector, currently two. An `&'a Arena` document borrows its arena,
  which suits scoped Rust callers but not objects a host keeps alive.
- Clones must behave as one allocator, as the `Allocator` contract requires.
  A quota allocator therefore keeps its counter behind the shared pointer.
- A failed reservation is `ErrorKind::Allocation` while parsing and
  `EditErrorKind::Allocation` while editing. Neither publishes partial state,
  so a bounded allocator is a safe way to cap memory under hostile input.
  Keep `Limits` and `EditLimits` as well: they bound work and the global-heap
  memory the allocator never sees.
- The allocator covers node records and attributes: 93–99% of retained bytes
  for markup-heavy documents in the parse benchmarks, 62% for one large text
  node. The string pool, ID and diagnostic lists, the DTD, parser working
  memory and all consumer output stay on the global heap. See
  [allocator strategy](design.md#allocator-strategy). XPath, C14N,
  validators and `Writer` accept any `A`. `save::to_vec`, HTML serialization
  and XInclude take global-heap documents only, and `parse_in` rejects
  `XINCLUDE`.

## Open decisions

- Whether a C ABI should imitate libxml2 symbol names. Its public structs
  expose mutable pointer graphs that an index arena cannot reproduce
  faithfully (see [design](design.md)). A yaxml-specific C API is simpler and
  safer.
- Packaging and CI matrices for each language.
