# Language binding boundary

Status: the C ABI (`bindings/capi`, crate `yaxml-capi`) and a Go package over
it (`bindings/go`) exist; see [C ABI](#c-abi) and [Go](#go). Python and Node
adapters are not written.

## Where unsafe lives

The `yaxml` crate forbids unsafe code and depends only on `memchr`. Any
foreign-function boundary needs unsafe code or a framework that contains it:
PyO3 for Python, magnus or a C ABI for Ruby, napi-rs or wasm-bindgen for
JavaScript, and cgo over a C ABI for Go. Bindings therefore live in **separate
crates**, such as `yaxml-capi`, `yaxml-py` and `yaxml-node`, that depend on
the safe core. The core's guarantees stay intact, and every unsafe line sits
in one audited adapter layer. The owner approved this
([decisions](#decisions-owner-2026-10-19)).

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
| Streaming output without a `Vec` | `save::to_writer` (`xmlSaveToIO`: any `io::Write`, a host file or stream object, in 32 KiB chunks; I/O failures come back as `WriteError::Io`); `TextWriter` (`xmlTextWriter`, one sink write per call) |
| Bulk results instead of one call per node | XPath `select_nodes` returns id vectors; serializers write whole documents to a `Write` |
| Errors that map onto host exceptions | `Error { kind, offset }`, described on demand as libxml2's `xmlError` ([Error mapping](#error-mapping)) |
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
4. Adapters expose libxml2's numbers: `Diagnostic::domain`, `code` and
   `level`, or `ErrorKind::libxml2_code` without the input. New variants
   only ever add rows.
5. Resolvers and XPath extension functions are the only places the core calls
   host code. Both are explicit, `Send` and fallible. A host exception becomes
   an `io::Error` or XPath error rather than unwinding through Rust.
6. Allocator-aware constructors (`allocator_api`, Rust 1.100) let adapters
   charge document memory to the host runtime's allocator; see
   [Allocators](#allocators). That is a memory-accounting choice, not a
   correctness dependency.

## Configuration structs

Every public `Limits`/`Options` struct (`Limits`, `DecodeLimits`,
`WriterLimits`, `EditLimits`, `ParseOptions`, and the `Limits`/`Options` of
`pattern`, `html`, `xsd`, `relaxng`, `schematron`, `catalog`, `xinclude`,
`xpath`, `save`, `c14n`) is `#[non_exhaustive]`: adding a field is not a
breaking change, but a struct literal, even with `..Default::default()`, no
longer compiles outside the crate. One convention applies to all of them:

- Fields stay `pub`, and every field has a chainable setter named after it
  (`src/setters.rs`, one `setters!(Struct { field: Type, .. })` per struct; a
  field missing from the list fails to compile): `Limits::DEFAULT.max_depth(64)
  .max_attributes(10)`. Setters are `const fn` except on `ParseOptions`, whose
  `Box<dyn Resolver>` and `String` fields have drop glue. A binding sets any
  field by name through its setter, or by assigning the pub field on a value it
  owns.
- `Default` and `pub const DEFAULT: Self` exist and agree, so statics and
  binding tables need no runtime call. `Default::default()` is
  `Self::DEFAULT`. `tests/defaults.rs` pins every default value; changing a
  default is a security decision.
- Types with no meaningful default take their required input in a `const fn
  new` (`c14n::Options::new(mode)`); `html::Options::new(flags)`,
  `save::Options::new(flags)` and `ParseOptions::new(flags)` are shorthands.
- There are no `with_*` builders; `ParseOptions::with_limits`,
  `with_decode_limits` and `with_resolver` are older composites.
- Defaults are the hostile-input policy. A binding exposes the field names
  as-is and never substitutes its own limits.

## Error mapping

The parser returns `Error { kind, offset }`: `Copy`, two words, built
without allocation, so failing costs nothing on the hot path. A binding that
raises a host exception describes it on demand from the input it parsed:

```rust
use yaxml::{Document, LineIndex};
let input = "<a>\n  <b></c>\n</a>";
let error = Document::parse(input).unwrap_err();
let lines = LineIndex::new(input.as_bytes());
let d = error.describe(&lines);
assert_eq!((d.domain as u8, d.code, d.level as u8, d.line, d.column), (1, 76, 3, 2, 10));
assert_eq!(d.message, "Opening and ending tag mismatch: b line 2 and c");
assert_eq!(d.render("a.xml", &lines).lines().next(),
           Some("a.xml:2: parser error : Opening and ending tag mismatch: b line 2 and c"));
```

`Diagnostic` is `xmlError` field for field: `domain` (`xmlErrorDomain`),
`code` (`xmlParserErrors`; the numbers yaxml produces are named in
`yaxml::diagnostic::code`), `level` (`xmlErrorLevel`), `line`, `column`
(`int2`), `message` (without libxml2's trailing newline), `str1`, `str2`,
`int1`, and `element` for the element `xmlError.node` names. `entity` marks
an error libxml2 raises in a nested entity's replacement text, where it has
no file and counts lines in that text. The file is the caller's, as is the
choice of host exception per domain or code.

How it works: `describe` scans the markup skeleton to the offset (open
elements, the construct holding the offset) and re-reads that construct the
way libxml2's parser does, so the first error found there, its position and
its wording (with tag, attribute and entity names and the open element's
line) are libxml2's. Errors in an internal entity's replacement text are
found by reading that text and reported where libxml2 reports them, after
the reference. The parser accepted everything before a strict error, so the
scan is exact up to the offset. `LineIndex` counts as libxml2 does (lines
end at `\n`; columns are characters from 1; a BOM is no column) and indexes
UTF-16 and single-byte input through its UTF-8 transcoding, so offsets stay
input bytes. Kinds the input cannot refine (limits, allocation, resolver and
XInclude failures, cancellation) use the context-free row of
`ErrorKind::libxml2_code`:

| `ErrorKind` | code | `ErrorKind` | code |
| --- | --- | --- | --- |
| `InvalidUtf8`, `InvalidUtf16`, `UnmappedByte`, `EncodingMismatch` | 81 (I/O) | `InvalidDtd` | 61 |
| `InvalidCharacter` | 9 | `ExternalResource` | 1549 (I/O) |
| `UnexpectedEof` | 77 | `XInclude` | 1609 (XInclude) |
| `Syntax`, `InvalidName` | 68 | `MissingRoot` | 4 |
| `MismatchedEndTag` | 76 | `MultipleRoots`, `TextOutsideRoot` | 5 |
| `DuplicateAttribute` | 42 | `InvalidDeclaration` | 57 |
| `Namespace` | 201 (namespace) | `UnsupportedEncoding` | 32 |
| `InvalidReference` | 8 | `UnsupportedOption` | 115 |
| `UnknownEntity` | 26 | `LimitExceeded`, `IdentifierExhausted` | 114 |
| `EntityLoop` | 89 | `Allocation` | 2 |
| `UnparsedEntity` | 28 | `Aborted` | 111 |
| `ExternalEntityInAttribute` | 29 | `InvalidState` | 1 |
| `UnbalancedEntity` | 85 | | |

The domain is the parser's unless noted. With the input, `describe` emits
72 codes, all named in `yaxml::diagnostic::code`: 65 parser codes of
libxml2's 1-118, the namespace codes 200-203 and 205, 1549 and 1609.

- `LineIndex::describe_all` turns a recovered document's `errors()` into
  libxml2's list: in order, without the "Premature end of data" and "Start
  tag expected" errors libxml2 raises only while the document is still
  well-formed.
- The re-reader is a second parser over untrusted input, so it has its own
  explicit budget rather than relying on the parser's limits having fired
  first: bytes scanned, re-read and expanded (entity replacement text,
  charged per expansion) count against 8 per input byte plus 64 KiB, shared
  by every error one `describe`/`describe_all` call handles. That is
  libxml2's default amplification factor of 5 for expansion plus three
  passes (skeleton scan, construct re-reads, declaration lookups). It is
  derived from the input rather than taken from `Limits` because describing
  needs no caller policy: a document the parser accepted with a raised
  `max_entity_amplification` only loses context on the errors past the
  budget, which then get the context-free code, a generic message and column
  0. The skeleton scan resumes across errors in offset order and never
  searches a construct's end twice; content-model nesting stops at libxml2's
  256, entity nesting at 20.
- `Diagnostic::render` prints `xmlFormatError`'s report: the
  `FILE:LINE: [element NAME: ]DOMAIN LEVEL : MESSAGE` line, then for parser,
  namespace and I/O errors `xmlParserInputGetWindow`'s context line and
  caret (and the bytes of an encoding error in unconverted input).
- The validators' diagnostics convert to the same shape: `describe(&lines)`
  on `valid`, `relaxng` and `xsd` diagnostics (domain `XML_FROM_VALID`,
  `RELAXNGV`, `SCHEMASV`; the element's `xmlGetLineNo` line; no column) and
  `describe()` on `schematron` ones, which carry their line.
- `Node::line` and `OwnedNode::line` are `xmlGetLineNo` (not capped at 65535,
  as with `XML_PARSE_BIG_LINES`); `TextReader::line_number` and
  `column_number` give the reader's parsing point. They take the
  `LineIndex` because neither tree nor reader keeps its input.
- There is no error callback (`xmlCtxtSetErrorHandler`,
  `xmlSetStructuredErrorFunc`) or `xmlGetLastError`: errors are returned, and
  an adapter's handler is a loop over `describe_all`.

Agreement with libxml2 2.15.4 (`tests/diagnostic_oracle.rs` against
`tooling/libxml2_errors.c`, which prints libxml2's structured errors):
libxml2's first error-level record and `describe` agree on code and line for
1063 of the 1065 XMLTS not-wf documents libxml2 rejects (99.81%) and on the
message for 1063 (99.81%); over `test/errors` on 38 of 41 (92.68%) for both.
Every disagreement is a parser difference rather than a description one:
yaxml fails elsewhere or not at all (an undeclared prefix inside an
entity's content, ISO-8859-5, which this libxml2 build lacks), or libxml2
switches to a declared UTF-16 decoder mid-declaration. Known description
gaps: libxml2's column counter skips each entity value's opening quote, so
its columns after one on the same line are one lower; attribute values whose
DTD type normalizes them are compared unnormalized; the context lines
`xmllint` prints for errors inside entities are left out.

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
  validators and `Writer` accept any `A`. `save::to_vec`/`to_writer`, HTML
  serialization and XInclude take global-heap documents only, and `parse_in` rejects
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

## Decisions (owner, 2026-10-19)

- **Adapters are approved, in separate crates** under `bindings/`. Unsafe code
  and binding dependencies (PyO3, napi-rs, cgo glue) stay there; the core crate
  remains `forbid(unsafe_code)` and memchr-only, and no binding dependency may
  enter its dependency graph or lockfile.
- **The C ABI is yaxml-specific**, not libxml2 symbol-compatible: libxml2's
  public structs expose mutable pointer graphs that an index arena cannot
  reproduce faithfully. Go (cgo) and C/C++ callers use it; Python and Node use
  native framework adapters over the safe core.
- **iconv-only encodings** (UCS-4, UCS-2, EBCDIC and others that distro
  libxml2 builds accept through iconv) are wanted, at low priority.

## C ABI

`bindings/` is its own Cargo workspace (`bindings/Cargo.toml`, own
`Cargo.lock`), so nothing it depends on reaches the core's graph. The core
is a single package, not a workspace, so its `cargo test` never builds
`bindings/`; its `exclude = ["/bindings"]` only keeps the bindings out of a
published core package.
`bindings/capi` builds `libyaxml_capi` as a cdylib, a staticlib and an rlib
(for its Rust tests and fuzzer). The header `include/yaxml.h` is
hand-written: smaller than a cbindgen dependency, and `tests/abi.rs` checks
that it declares exactly the exported functions and pins every struct's size
and key offsets, which `tests/c/abi_test.c` pins again with `_Static_assert`.

```c
#include "yaxml.h"

yaxml_doc *doc;
yaxml_error err = {0};
if (yaxml_parse((const uint8_t *)xml, len, NULL, &doc, &err) != YAXML_OK) {
  printf("%llu:%llu: %.*s\n", err.diagnostic.line, err.diagnostic.column,
         (int)err.diagnostic.message.len, err.diagnostic.message.ptr);
  yaxml_free(err.diagnostic.message.ptr);
  return;
}
yaxml_node root;
yaxml_doc_element(doc, &root, NULL);
yaxml_xpath_value v;
yaxml_str expr = {"//item", 6};
if (yaxml_xpath_eval(doc, root, expr, NULL, 0, &v, &err) == YAXML_OK) {
  for (size_t i = 0; i < v.len; i++) { /* v.items[i].node */ }
  yaxml_free(v.items);
}
yaxml_doc_free(doc);
```

**Calls.** Every function returns a `yaxml_status`: `YAXML_OK` (0),
`YAXML_NONE` (1: the node or value asked for does not exist; out-parameters
get null values) or a negative error, stable forever:

| Status | Meaning | `err->diagnostic` |
| --- | --- | --- |
| `ERR_NULL` -1 | a required pointer is NULL | domain 0 |
| `ERR_ARGUMENT` -2 | invalid UTF-8, unknown enum value, `struct_size` too small or unknown nonzero trailing fields | domain 0 |
| `ERR_HANDLE` -3 | unknown, freed, stale, foreign or forged handle or node | domain 0 |
| `ERR_BUSY` -4 | the handle is in use (see threads) | domain 0 |
| `ERR_POISONED` -5 | an earlier call panicked on this handle | domain 0 |
| `ERR_PANIC` -6 | this call panicked (a yaxml bug); the handle is now poisoned | domain 0, message `internal error: ...` |
| `ERR_XML` -7 | parse or serialization error | libxml2's: `Error::describe` against the input (parse), `ErrorKind::libxml2_code` otherwise |
| `ERR_EDIT` -8 | editing error | domain 1000, code `YAXML_EDIT_*` (one per `EditErrorKind`), column = UTF-8 index + 1 |
| `ERR_XPATH` -9 | XPath error | domain 12, code 1200 + `xmlXPathError`, libxml2's message (`xpath::Error::libxml2`) |
| `ERR_SCHEMA` -10 | a schema did not compile | domain 16 (XSD), 18 (RELAX NG) or Schematron's, first error's code, every error in the message |
| `ERR_C14N` -11 | canonicalization error | domain 21, codes 91, 1953, 1955, 1546 |
| `ERR_CALLBACK` -12 | a callback stopped the parse or a write failed | domain 0 |

`err` may be NULL. On an error it is filled (status, input byte offset of a
parse error, diagnostic) and nothing else is written; its message is owned.
On success it is untouched.

**Handles.** `yaxml_doc*`, `yaxml_reader*`, `yaxml_push*` and
`yaxml_schema*` are tokens, never Rust pointers: each indexes a table of live
objects, tokens come from one counter and are never reused. A freed, stale or
invented handle is `ERR_HANDLE`, a second free returns `ERR_HANDLE` and does
nothing, and a handle of one type passed as another is not found. Nodes are
values, `yaxml_node { doc, slot }` = `OwnedNodeId::parts()`, rebuilt with
`OwnedNodeId::from_parts` and validated on every call; a removed node, a node
of another document (including a clone) and a forged slot are `ERR_HANDLE`.
`{0, 0}` is the null node. XPath results and C14N node-sets are
`yaxml_xpath_item { node, type, index }`: type 0 is a tree node, 2 an
attribute (index = position in `yaxml_node_attribute_at` order), 18 a
namespace (index on XPath's namespace axis, 0 = `xml`).

**Memory.** Strings are UTF-8 `yaxml_str { ptr, len }`, never
NUL-terminated. In arguments, `ptr == NULL` (with `len == 0`) means absent
where the header says so (no namespace, default prefix, document encoding).
Results come in two kinds:

- *Borrowed* `yaxml_str`: from a document they point into its string pool
  and stay valid until the next editing call on that document or
  `yaxml_doc_free`; from `yaxml_reader_next` until the next call on that
  reader; in an event callback until the callback returns.
- *Owned*: `yaxml_buffer`, every array (`yaxml_diagnostics.items`, which also
  holds the messages, `yaxml_xpath_value.items` and `.string`) and every error
  message. `yaxml_free(ptr)` frees each; NULL, unknown and already freed
  pointers are ignored, so a double free on an error path is harmless. Empty
  results are NULL and own nothing.

Owned results are kept in a table until freed, so leak checkers see them as
reachable: `yaxml_live_objects(&handles, &allocations)` counts live handles
and unfreed allocations, and a leak test checks both are 0 at exit (the C
test, the fuzzer and the Go tests do).

**Options.** `yaxml_parse_options` and `yaxml_edit_limits` start with
`uint32_t struct_size` (set it to `sizeof`). Fields are only appended: the
library reads the caller's `struct_size` bytes, a smaller struct than the
first version is `ERR_ARGUMENT`, fields the caller's header lacks keep their
defaults, and bytes past the fields the library knows must be zero (a newer
caller's nonzero option is refused rather than ignored). `flags` are
libxml2's `XML_PARSE_*` values; every limit field is a `Limits`,
`DecodeLimits` or `EditLimits` field of the same name, and 0 keeps yaxml's
default, so a zeroed struct is the hostile-input policy. NULL options are
the defaults. No resolver crosses the ABI: nothing external is ever loaded.

**Panics and poisoning.** Every exported function runs inside
`catch_unwind`; a panic becomes `ERR_PANIC` with the panic message. Access to
a handle goes through a guard that marks the handle poisoned if the access
unwinds, so later calls on it are `ERR_POISONED` while its free still works.
Callbacks are `extern "C"` and must not unwind (a C++ exception or
`longjmp` out of one aborts or is undefined, as for any C library).

**Threads.** Any handle may be used from any thread (`OwnedDocument` is
`Send + Sync`; the parsers are reached only through `&mut`). Each object sits
behind a reader-writer lock taken with `try_read`/`try_write`: concurrent
read-only calls on one document run in parallel, and a call that would wait
(an edit during another call on the document, a callback re-entering the
parser or document it was called from) returns `ERR_BUSY` instead, so no
call blocks or deadlocks.
Freeing a handle while another thread uses it is safe: the call in flight
keeps its object alive. Borrowed strings are the caller's to synchronize
with edits.

**Functions.**

| Area | Functions |
| --- | --- |
| Library | `yaxml_version`, `yaxml_has_feature` (`XML_WITH_*`), `yaxml_free`, `yaxml_live_objects` |
| Documents | `yaxml_parse` (copies the input, kept for line numbers and diagnostics), `yaxml_doc_create`, `yaxml_doc_clone` (`xmlCopyDoc`), `yaxml_doc_free`, `yaxml_doc_root` (document node), `yaxml_doc_element`, `yaxml_doc_element_by_id`, `yaxml_doc_diagnostics` (`RECOVER` and `DTDVALID` diagnostics), `yaxml_doc_set_edit_limits` |
| Nodes | `yaxml_node_kind` (`xmlElementType`), `yaxml_node_relative` (parent, first/last child, next/previous sibling), `yaxml_node_name` (also PI target, entity name), `yaxml_node_value`, `yaxml_node_content` (`xmlNodeGetContent`), `yaxml_node_path` (`xmlGetNodePath`), `yaxml_node_line` (`xmlGetLineNo`), `yaxml_node_attribute_count`/`_at`, `yaxml_node_attribute` (`xmlGetProp`), `yaxml_node_attribute_ns` (`xmlGetNsProp`), `yaxml_node_namespace_uri` (`xmlSearchNs`), `yaxml_node_namespace_prefix` (`xmlSearchNsByHref`) |
| XPath | `yaxml_xpath_eval`: node-set, boolean, number or string, with prefix bindings |
| Output | `yaxml_save` (`xmlSaveDoc` for the document node, else `xmlNodeDump`; `XML_SAVE_*` flags, encoding), `yaxml_save_to` (the same through a write callback, in 32 KiB chunks), `yaxml_c14n` (three modes, comments, node-set, inclusive prefixes) |
| Validation | `yaxml_validate_dtd`, `yaxml_schema_new` (XSD, RELAX NG, Schematron; nothing referenced is loaded), `yaxml_validate`, `yaxml_schema_free`; diagnostics as `yaxml_diagnostics` arrays of `yaxml_diagnostic` (domain, code, level, line, column, message) with an `errors` count |
| Editing | `yaxml_append_element`, `yaxml_append_leaf` (text, CDATA, comment, PI), `yaxml_set_value`, `yaxml_set_attribute`, `yaxml_remove_attribute` (`NONE` when absent), `yaxml_declare_namespace`, `yaxml_set_name`, `yaxml_set_url`, `yaxml_move` (append child, insert before/after, replace), `yaxml_unlink`, `yaxml_remove`, `yaxml_copy`, `yaxml_import` (from another document; the same one is `ERR_BUSY`), `yaxml_compact` |
| Streaming | `yaxml_reader_new` (copies the input; `BufferedReader`), `yaxml_reader_next` (`NONE` at the end), `yaxml_reader_free`; `yaxml_push_new`, `yaxml_push_feed`, `yaxml_push_finish`, `yaxml_push_free` (`PushParser`; the callback gets a borrowed `yaxml_event` and a user pointer, and a nonzero return stops with `ERR_CALLBACK`) |

`yaxml_event` carries the kind, name, value, target, system id, internal
subset, attributes (borrowed array) and input byte range (`UINT64_MAX`
without one); the header's kind table says which field holds what.

**Unsafe code.** All of it is in `src/ffi.rs`: `slice` (pointer and length
to a slice, after NULL, alignment and size checks), `get`, `text` (plus
UTF-8 validation), `versioned` (the `struct_size` read), `Out::put`/`reply`
(out-parameter writes after NULL and alignment checks), `emit` and `write`
(callback calls), and `Send`/`Sync` for C records stored beside the strings
they point to. Exported functions are `unsafe extern "C" fn`; each call of a
helper carries a `// SAFETY:` comment naming the yaxml.h contract it relies
on. The crate denies `unsafe_op_in_unsafe_fn` and clippy's
`undocumented_unsafe_blocks` and `multiple_unsafe_ops_per_block`.

**Testing.** `cargo test` in `bindings/` runs the Rust tests (every function,
happy and sad paths, layouts, header symbols, panic poisoning, busy
handling). `bindings/capi/tests/c/run.sh` builds `tests/c/abi_test.c`, which
calls every function and frees everything, with
`-fsanitize=address,undefined` and runs it (LeakSanitizer on Linux), then on
macOS runs an uninstrumented build under `leaks --atExit`; both end with
`yaxml_live_objects` at 0. `bindings/capi/fuzz` (cargo-fuzz, own workspace,
nightly-2026-10-18) has the `calls` target: random call sequences through the
exported functions with NULL out-parameters and errors, freed, stale,
invented and cross-type handles, forged nodes, double frees, invalid UTF-8,
truncated `struct_size`, and callbacks that stop or fail; it reads every
returned string under ASan and checks the tables are empty after each input:

```sh
cd bindings/capi/fuzz && cargo fuzz run -O calls -- -max_total_time=600
```

Core accessors added for the ABI: `OwnedNodeId::parts`/`from_parts` (node
handles as integers, still validated by every lookup) and
`xpath::Error::libxml2` made public (XPath errors as libxml2's code and
message).

## Go

`bindings/go` is the module `github.com/pbdeuchler/yaxml/bindings/go`
(package `yaxml`, Go 1.26), cgo over the static library:

```sh
cargo build --release --manifest-path bindings/Cargo.toml -p yaxml-capi  # or go generate
cd bindings/go && go test ./...
```

```go
doc, err := yaxml.Parse(data, yaxml.WithFlags(yaxml.NoEnt))
if err != nil {
	var d yaxml.Diagnostic
	errors.As(err, &d) // libxml2's domain, code, level, line, column, message
	return err
}
defer doc.Close()
root, _ := doc.Root()
for child := range root.Children() {
	fmt.Println(child.Name().Local, child.Text())
}
items, err := doc.XPath("//p:item/@id", yaxml.Namespace{Prefix: "p", URI: "urn:p"})
out, err := doc.Save(yaxml.Format)
```

- `Parse(data, ...Option)` with `WithFlags`, `WithBase`, `WithLimits`;
  `Create(Name)`; `Document.Close` (idempotent; `runtime.AddCleanup` frees a
  forgotten document as a safety net), `Clone`, `Root`, `DocumentNode`,
  `ElementByID`, `Diagnostics`, `ValidateDTD`, `XPath`, `Save`, `WriteTo`
  (`io.WriterTo`, through the write callback), `C14N`, `Import`.
- `Node` is a value (document plus handle): `Kind`, `Name`, `Value`, `Text`,
  `Path`, `Line`, `Attrs`, `Attr`, `AttrNS`, `LookupNamespace`, `Parent`,
  `FirstChild`, `LastChild`, `NextSibling`, `PreviousSibling`, `Children`
  (`iter.Seq`), `Eval` (any XPath value), `Save`, and the edits
  (`AppendElement`, `AppendText`, `AppendCDATA`, `AppendComment`,
  `AppendPI`, `SetValue`, `SetAttr`, `RemoveAttr`, `Rename`, `AppendChild`,
  `InsertBefore`, `InsertAfter`, `Replace`, `Unlink`, `Remove`, `Copy`).
  XPath attribute results are attribute nodes (`Kind() == AttributeNode`).
  A node of a closed document reads as empty and its edits fail.
- `CompileSchema(XSD|RelaxNG|Schematron, data, base)`,
  `Schema.Validate(doc)`, `Valid(diagnostics)`.
- `NewReader(data, ...Option)` and `Reader.Next` (`io.EOF` at the end):
  events copied into Go, names interned per reader (at most 4,096).
- Errors are `*Error{Status, Offset, Diagnostic}`; `Unwrap` gives the
  `Diagnostic`. `LiveObjects()` exposes `yaxml_live_objects`; `TestMain`
  fails if a test leaks.
- Strings are copied both ways; strings stored in structs passed to C
  (names, XPath bindings) are pinned with `runtime.Pinner`. The tests pass
  under `-race` and `GOEXPERIMENT=cgocheck2`.

**FFI overhead** (`go test -bench . -benchmem`, `bench_test.go`; Apple M-series,
2026-10-20, medians of 5): a cgo call costs about 37 ns, and the copy into
Go is the rest.

| Input | yaxml DOM (`Parse`+`Close`) | yaxml `Reader` (all events into Go) | `encoding/xml` `Token` loop |
| --- | --- | --- | --- |
| 10-byte `<r a="1"/>` | 1.39 µs, 10 allocs | 1.95 µs, 15 allocs | 0.39 µs, 10 allocs |
| 100 KB, 1,000 namespaced items (9,003 events) | 0.70 ms (142 MB/s), 10 allocs | 2.52 ms (39 MB/s), 5,032 allocs | 1.82 ms (56 MB/s), 27,025 allocs |

The DOM is one call per document, so a large document parses 2.6x faster
than `encoding/xml` tokenizes it; a tiny one pays a fixed 1 µs (handle
table, options, input copy, cleanup registration). The `Reader` pays a call
and a copy per event: of its 280 ns per event, the core's `BufferedReader`
is 90, the ABI (handle lookup, event lowering into reused buffers) 40 and
the Go side (cgo call, string copies; names are interned) 150. Code that
walks a parsed document or asks XPath for nodes stays on the DOM side.

## Open decisions

- Packaging and CI matrices for each language beyond the Linux CI jobs
  (`capi`, `capi-fuzz`, `go`); a Python or Node adapter.
