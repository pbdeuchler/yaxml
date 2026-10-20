# Structural libxml2 differential protocol

`structure_oracle` compares the owned DOM's leaf insertion and permanent subtree
removal with an external libxml2 **2.15.4** helper. The helper is
[`yaxml-harness/tooling/libxml2_structure.c`](https://github.com/pbdeuchler/yaxml-harness/blob/main/tooling/libxml2_structure.c); Cargo never
compiles or links it. The library remains safe Rust with only `memchr` as a
dependency. The pinned archive, checksum and shared subprocess restrictions are
documented in [oracle.md](oracle.md).

```sh
cc -std=c11 -Wall -Wextra -Werror \
  -Itarget/oracle/build -Itarget/oracle/libxml2-2.15.4/include \
  tooling/libxml2_structure.c target/oracle/build/libxml2.a \
  -lm -pthread -ldl -o target/oracle/structure-helper
cargo run --release --example structure_oracle -- \
  --libxml2-helper "$PWD/target/oracle/structure-helper"
```

This build command targets the Ubuntu static build used by CI. Other platforms
must use their libxml2 installation's include paths and linker flags. Optional
positional case names select individual fixtures. `--allow-other-libxml2` labels
the run exploratory; without it both the compiled and runtime versions must be
`21504`. Tree and output support are required. The executable path, version,
mode, case dispositions and final counts are reported. CI retains these records,
the Rust and C compiler versions, revision, build options and helper/catalog
source hashes. A fake helper is never a libxml2 compatibility result.

Each invocation reparses the original input independently and performs exactly
one operation. This avoids a persistent pointer registry and makes pre-operation
state independently observable. The catalog has 34 cases: 23 exact comparisons,
three projected comparisons, six intentional differences and two explicit
non-comparisons. This bounded conformance runner is not a benchmark or a
multi-operation model; the Rust mutation fuzz target covers sequences, stale
handles, reopening editors and quota rollback separately.

| Disposition | Contract |
| --- | --- |
| Exact | Initial and final kinds, hierarchy, order, qualified names, namespace URIs, ordinary attributes and direct leaf values agree. Serialized bytes also agree. |
| Projected | Text append can return the preceding Text sibling after coalescing and freeing the newly supplied C node. Maximal adjacent Text runs are compared; CDATA is preserved. Empty PI serialization has a separate literal C-byte expectation (`<?p ?>`) rather than Rust's `<?p?>`. |
| Intentional difference | Rust must reject with the specified error and preserve its full tree and bytes. C must produce the separately predicted result. These cases cover root unlink, append to a Text parent, document-level Text, malformed comments, reserved PI target and leading PI whitespace. They do not count as agreement. |
| Not compared | Allocation/OOM, foreign or stale handles and ownership failures have different contracts. Namespace moves, adoption and repair are outside this API slice. |

Fixtures cover every leaf kind, empty leaves, Text/CDATA boundaries, Unicode and
escaping, append before an existing epilog, removal at each sibling position,
prolog/epilog removal, namespace-bearing subtree removal, and BOM UTF-16LE/BE
sources. Removing a comment between two Text nodes must leave two distinct Text
nodes in the C tree; the Text-run projection is only used for append cases.
Fresh Rust leaves must have a new live handle and no source provenance. Removed
Rust descendants must become stale and surviving handles must remain live.
These Rust-only assertions are not attributed to libxml2.

The C side calls `xmlNewDocText`, `xmlNewCDataBlock`, `xmlNewDocComment` or
`xmlNewDocPI`, followed by `xmlAddChild`. It checks the returned live pointer
against the parent or previous sibling when coalescence is expected, and never
inspects the supplied pointer after insertion. For removal it calls
`xmlUnlinkNode`, snapshots and serializes the survivors, then calls `xmlFreeNode`
on the detached subtree. It never inspects a freed node. The document remains
alive until detached nodes are freed, preserving dictionary ownership.

These choices follow the pinned [tree implementation](https://raw.githubusercontent.com/GNOME/libxml2/v2.15.4/tree.c).
libxml2 permits several lexical or placement states rejected by the Rust editor;
its constructors are not a validating equivalent of the Rust API. Cross-document
namespace reconciliation and allocation-failure recovery require different tests.
An allocation, constructor, insertion, save or helper failure is inconclusive,
never a successful rejection or compatibility agreement.

The response contains direct C tree values. It is not reparsed with yaxml, so
shared Rust parser/serializer bugs cannot establish agreement by themselves.
Serialization uses `xmlSaveTree` for each top-level child with UTF-8 and
`XML_SAVE_NO_DECL | XML_SAVE_AS_XML | XML_SAVE_EMPTY`, avoiding document-level
declaration, separator and final-newline additions. Exact bytes are asserted only
for catalog fixtures with overlapping spelling contracts. This is not a general
claim of byte-identical libxml2 serialization.

## Wire format and bounds

The protocol identifier is `YAXML-STRUCTURE-1`. Decimal numbers end in LF.
A byte frame is decimal byte length, colon, exactly that many bytes and LF;
framing preserves embedded LF, CR, tab, colon and multibyte UTF-8. Lengths are
bytes, unlike the XPath protocol's Unicode scalar counts.

The request contains the identifier, action number (1 Text, 2 CDATA, 3 comment,
4 PI, 5 remove), original preorder target ordinal, and three frames: original
XML bytes, PI target and value. Document ordinal is zero. Source bytes are
limited to 64 KiB, the target node ordinal to 255, and the PI target and value
frames to 4096 bytes each. C-string
payloads cannot contain NUL; original UTF-16 source may. Extra or truncated input
is rejected. The CLI exposes only the fixed catalog, not arbitrary XML requests.

The response contains the identifier, the initial snapshot, returned-node class
(0 new, 1 previous Text sibling, 2 Text parent, 3 unlinked), and the final
snapshot. Each snapshot starts with its actual node count, including the
document. Each node records kind (`D`, `E`, `T`, `C`, `M`, `P`), parent ordinal
plus one (zero for the document), qualified name or PI target, namespace URI,
direct value, actual ordinary attribute count, and ordered attribute records
(qualified name, namespace URI, normalized value). A final frame holds serialized
UTF-8 bytes. Actual counts detect extra nodes and attributes; trailing data is
rejected. Parent ordinals must refer to an earlier node.

Each snapshot is limited to 256 nodes and 4096 ordinary attributes, and total
response size to 8 MiB. Each helper process has the shared five-second deadline
and combined 8 MiB stdout/stderr ceiling. Bad framing, nonzero exit, unexpected diagnostics,
timeout or output overflow cannot count as agreement. Any disagreement or
inconclusive case fails the command. Skips and intentional differences have
separate counts.

Declaration metadata, original encodings, source spans, XMLNS attribute placement
and unused namespace bindings are absent from the semantic projection. Ordinary
attributes retain source order, qualified names and effective namespace URIs.
Exact serialized fixtures provide additional lexical coverage without expanding
the semantic contract. The helper rejects DTD-bearing documents after parsing;
it is a trusted fixed-fixture tool, not an untrusted-input sandbox. Its C code
and dependency ownership rules are outside the Rust library's safety guarantee.
The subprocess utility clears catalog environment variables, uses regular-file
I/O, caps retained output and kills the direct child on deadline. It does not
sandbox an arbitrary replacement executable or its descendants.

## Evidence

Normal Cargo tests compile a standalone safe-Rust fake helper and exercise the
actual CLI. They check exact, projected, intentional and skipped counts;
compiled/runtime version mismatches; malformed, truncated and trailing protocol;
before/after tree and byte mismatches; wrong returned-node classification;
diagnostics and failed helper exit. Literal protocol unit tests also cover
Unicode/control framing, ordered namespace-bearing attributes and every response
truncation. These tests verify failure gates without installed libxml2 headers.

Local exploratory execution against Homebrew libxml2 **2.15.3** passed the full
catalog, including an AddressSanitizer/UndefinedBehaviorSanitizer helper run.
The checksummed **2.15.4** source was then built locally and the pinned catalog
passed with 23 exact agreements, three documented projections, six verified
intentional differences, two explicit non-comparisons and no disagreement or
inconclusive result. The [structural validation report](archive/validation-owned-structure.md)
records build options, hashes and raw evidence. CI runs the same pinned catalog;
the hosted job has not been executed in this local session.
