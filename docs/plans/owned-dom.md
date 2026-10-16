# Owned DOM implementation plan

Build an immutable UTF-8 DOM whose strings and topology survive destruction of
the source. Preserve the borrowing `Document` API and its current representation.
This is the next P1 storage step; UTF-16 decoding and binding registries remain
separate work because they need original-byte offset and ownership contracts.

## Representation and API

Add `OwnedDocument`, `OwnedNode` and a non-exhaustive `OwnedNodeKind` tag with
Document, Element, Text, Cdata, Comment and ProcessingInstruction variants.
Owned records contain indices/ranges, never references into their containing
document. One UTF-8 pool holds logical strings; name records store qualified
spelling once and derive prefix/local slices. Original source ranges are kept
separately from pool ranges. The source itself need not be retained.

`OwnedDocument::{parse, parse_bytes, with_limits}` build from validated Reader
events. Expose root/root_element/node/nodes/declaration with the borrowing DOM's
navigation and source-range semantics. `OwnedNode` is a Copy view with id, kind,
name, text, attributes, range, parent/first_child/last_child/next_sibling,
children/descendants, and processing_instruction_target. Names and declarations
are returned by value, borrowing the pool. Attributes use an exact-size iterator
of existing `Attribute` views, with borrowed Cow values and namespaces; no view
operation allocates. Node IDs remain document-local with no identity guarantee.

Reuse a small private generic record/append helper for topology if it removes
real duplication. Do not expose storage generics or alter existing name fields.
Do not add a mutable tree, generic DOM framework or speculative interning layer.

## Bounds and failure

Use the same Limits profile and semantic engine as the borrowing reader. Extend
private budget access so parser normalization and owned pool reservations charge
one operation's max_owned_bytes. Charge the full requested new pool capacity
before each growth, including prior buffers already released; use geometric
growth without a large minimum reservation, and cap it by remaining budget.
Empty copies need no reservation. Retained byte policy counts allocation work,
not total process memory; nodes, attributes and allocator overhead need their
own eventual accounting. All input-sized collection growth is fallible.

An owned DOM can exhaust its string budget before a borrowing DOM. Such a limit
failure is explicit, with a valid source offset; grammar acceptance otherwise
agrees. Namespace URI copies can amplify retained values relative to source
size, so apply the budget before copying. Any repeated-URI optimization must
preserve stable range validity and receive adversarial size-series evidence.
No successful partial tree escapes an error. Parsing, traversal and destruction
remain iterative. No unsafe code or dependency is added.

## Acceptance and review

- Independent logical goldens for declarations, every node kind, names,
  namespaces, attributes, references and line-ending normalization.
- Drop/overwrite input and move the document; force pool growth with Unicode;
  verify every view, source range and navigation link afterward.
- Compare complete borrowed/owned trees and writer output across the corpus.
- Accepted/rejected input and each resource ceiling; exact aggregate budget
  regressions distinguishing parser-only and retained storage charges.
- Deep/wide traversal and iterative destruction; out-of-range IDs; Send + Sync.
- Map every new input/view API into arbitrary-byte fuzz checks with explicit
  owned-budget exceptions; preserve value and topology assertions.
- Run formatting, strict Clippy, all targets/doctests, fuzz-only source coverage
  and an independent clean-tree mutation review before local main integration.

Performance superiority, Rust 1.100 allocator support, UTF-16, full libxml2
compatibility and 100% fuzz execution coverage remain separate open goal items.
