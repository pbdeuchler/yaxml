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
| A DOM that host code edits in place | `Editor`: `append_child`/`insert_before`/`insert_after`/`unlink`/`replace`, `set_attribute`/`remove_attribute`, `set_name`, `declare_namespace`, `set_url`; each call is atomic and keeps namespaces declared where used, so a wrapper never reconciles ([owned-dom.md](owned-dom.md#moving-unlinking-and-replacing)) |
| Host objects for unlinked nodes | `unlink`/`replace` leave detached subtrees whose handles stay live until reinserted or `remove_subtree`d; a wrapper object needs no ownership transfer, only its handle |
| Long-lived documents under repeated edits | `Editor::compact` reclaims pool and attribute garbage without changing any handle; removed slots stay as one tombstone record each |
| Copying nodes and documents (`copy.copy`, `deepcopy`, cross-tree append) | `Editor::import` (from any document, any allocator), `Editor::copy` (within one), `OwnedDocument::try_clone`; copies come back detached and namespace-consistent, with every reservation made before anything is published ([owned-dom.md](owned-dom.md#copying-and-importing)) |
| DOM accessors hosts expect | `attribute`/`attribute_ns`, `content`, `list_string`, `path`, `base_uri`, `lang`, `space_preserve`, `search_ns`, `namespaces`, `previous_sibling` on `Node` and `OwnedNode` |
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

## Capabilities, versions and reuse

- `yaxml::features::has(Feature)` / `has_code(u32)` answer `xmlHasFeature`
  and keep libxml2's `XML_WITH_*` numbers (`Feature as u32`), so a binding
  passes the int through. `all()` lists every `(Feature, bool)`; unknown
  numbers and `XML_WITH_NONE` are false. `ALLOCATOR_API` reports the cargo
  feature of that name. A binding should map its own module probes
  (`HAVE_XPATH`, ...) onto these instead of hard-coding.
- `yaxml::VERSION` is the crate version; `LIBXML2_COMPAT_VERSION` ("2.15.4")
  and `LIBXML2_COMPAT_VERSION_NUMBER` (21504, `LIBXML_VERSION`) say which
  libxml2 is matched; `features::check_version(v)` is `xmlCheckVersion`
  returning a bool instead of printing.
- `PushParser::reset`/`reset_with(options)`, `BufferedReader::reset(input)`/
  `reset_with(input, options)` and `TextReader` likewise return the previous
  source and keep allocation capacity (tokenizer buffer, namespace stack,
  name buffers). Everything else starts over: positions, budgets
  (`Limits` counters), sticky XML and I/O failures, finish state, DTD and
  entity state, queued nodes, schemas, and the resolver. `reset()` keeps the
  flags and limits but not the resolver; `reset_with` takes a full
  `ParseOptions`, rejects unsupported options before changing anything, and
  is how `xmlCtxtSetOptions` is spelled. Retained capacity is what the previous document needed, which `Limits`
  bound (the tokenizer buffer by `max_token_bytes`); drop the parser to
  release it.
  Measured: 100k tiny documents (47 bytes, 6 events) 820 ns fresh vs 690 ns
  reset (-16%), a 4-byte document 230 vs 200 ns (`examples/reset_bench.rs`).

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
  storage vector, currently three. An `&'a Arena` document borrows its arena,
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

## Moving nodes between documents

lxml lets a program append an element of one tree into another; libxml2
moves the node (`xmlDOMWrapAdoptNode`) and every Python proxy keeps pointing
at the same `xmlNode`. yaxml cannot keep handles across documents: an
`OwnedNodeId` is a document identity plus a slot in that document's arena, so
a node in another arena is a different node. There is therefore no `adopt`;
a move is two calls on two editors, which Rust allows because the documents
are different values:

```rust,ignore
let mut from = source.edit(limits)?;
let mut to = destination.edit(limits)?;
let moved = to.import(from.document(), node, true)?; // on failure nothing changed
from.remove_subtree(node)?;                         // fails only for the document element
to.append_child(parent, moved)?;
```

`remove_subtree` fails only on the synthetic root and the document element,
which the adapter checks first, so the move is atomic in practice; if it
fails, removing the detached copy undoes the import. A document keeps its
element, so moving another tree's root element is a copy. `append_child`
returns the handle that holds the node: a Text merged into the last child
returns that child. An adapter remaps its
proxies by walking the source subtree and the copy's `descendants()`
together before the removal: they visit corresponding nodes in the same
order, except that a reference to an entity the destination does not declare
is replaced by its content (the reference itself has no counterpart). The
source's handles then become stale, which is what rejects a proxy the
adapter forgot to remap. A single `adopt` call would save one line and hide
which handles died, so it is left to the adapter.

## Open decisions

- Whether a C ABI should imitate libxml2 symbol names. Its public structs
  expose mutable pointer graphs that an index arena cannot reproduce
  faithfully (see [design](design.md)). A yaxml-specific C API is simpler and
  safer.
- Packaging and CI matrices for each language.
