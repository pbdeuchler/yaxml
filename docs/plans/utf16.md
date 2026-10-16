# UTF-16 input implementation plan

Add strict UTF-16LE/BE input to the default PushParser and OwnedDocument byte
interfaces. Preserve the borrowing Reader/Document UTF-8-only contract and its
existing behavior. Keep UTF-8 bulk scanning and the Reader-based owned fast path.
No complete decoded input or whole-input offset map is retained.

## Boundary and public API

One private encoding boundary owns BOM/signature detection and UTF-16 conversion.
It retains at most four signature bytes, an incomplete code unit and a pending
high surrogate. UTF-8 forwards borrowed chunks; UTF-16 emits complete scalars
through a stack buffer. Decoder finish checks truncation before XML finish.
Do not strip a decoded U+FEFF a second time in the tokenizer.

Add DecodeLimits with max_decoded_bytes (64 MiB default) and
max_decoded_token_bytes (1 MiB default). Existing Limits fields/construction
remain unchanged: input/token bytes count original source, including BOM in total
input but excluding it from a lexical token. Decoded counters exclude transport
BOMs. New constructors:

- PushParser::with_decode_limits(limits, decode_limits)
- OwnedDocument::with_decode_limits(input, limits, decode_limits)

Existing constructors use default DecodeLimits. Add Limit::DecodedBytes and
DecodedTokenBytes plus ErrorKind::InvalidUtf16 and EncodingMismatch. Keep new
categories non-exhaustive. No extra dependency or unsafe code is permitted.

## Detection and declaration policy

UTF-16 BOMs select byte order; an encoding declaration is optional afterward.
Recognize four-byte UTF-32 signatures before accepting an ambiguous two-byte
BOM and reject UTF-32 explicitly. BOM-less 00 3C 00 3F / 3C 00 3F 00 signatures
select a provisional order and require a leading XML declaration naming matching
UTF-16BE / UTF-16LE. Generic UTF-16 without a BOM is rejected.

Accept registered UTF-8, UTF-16, UTF-16LE and UTF-16BE labels case-insensitively
in the relevant contexts; do not add libxml2's unhyphenated legacy aliases yet.
Preserve the original declaration fields. Keep existing UTF-8-context unsupported
encoding classifications; a conflicting supported declaration after UTF-16
detection is EncodingMismatch, and unknown labels remain UnsupportedEncoding.
Malformed declaration syntax remains InvalidDeclaration before agreement checks.
Matching endian-specific labels with BOMs are accepted as an explicit tolerance.

Strict mismatch rejection differs from libxml2's autodetected warning behavior.
Record this as a deliberate compatibility difference, with no claim of a new
pinned binary pass. References: [XML encoding rules](https://www.w3.org/TR/2008/REC-xml-20081126/#charencoding),
[autodetection](https://www.w3.org/TR/2008/REC-xml-20081126/#sec-guessing),
[RFC 2781](https://www.rfc-editor.org/rfc/rfc2781#section-3.3),
[pinned libxml2 detection/declarations](https://raw.githubusercontent.com/GNOME/libxml2/v2.15.4/parserInternals.c).

## Coordinates, limits and ownership

Tokens carry decoded UTF-8 and original source ranges. Semantic parsing uses
decoded coordinates; public events, errors and nodes use original byte offsets.
Translate a semantic failure once using the unnormalized token prefix and UTF-16
code-unit lengths. Decoder errors already identify their original code unit;
EOF errors use raw progress. Ignored whitespace still advances both coordinates.
No per-name/attribute remapping or global source map is needed.

Raw token, decoded token and decoded total ceilings must be enforced before
retaining excess data. Keep text's delimiter lookahead out of the preceding
token's length. Position counts consumed original bytes, including pending
signature/code-unit bytes and one scalar of text lookahead; a chunk suffix may
remain unconsumed after an error. Preserve terminal errors, cancellation and
poisoning across callbacks or fallible consumers. Decoder/source failures must
not become Aborted.

Extract a concrete private owned-tree builder consuming event, source range and
the shared Budget. UTF-8 uses Reader events as before; UTF-16 uses a private
fallible PushParser consumer. Pool growth, normalization and retained streaming
names share one operation budget. Builder errors propagate unchanged. Retain
flat parsing/traversal/destruction and allocation-free borrowed views.

## Evidence and integration

Independent tests cover both orders and all byte splits; BOM/signature ambiguity;
surrogate boundaries and odd EOF; repeated BOMs; declaration combinations and
spelling; Unicode before malformed names/references; CRLF normalization; raw
spans and positions; cancellation/panics/I/O; exact raw, decoded and aggregate
limits. Compare logical UTF-16 results with independently encoded UTF-8 fixtures.

Update fuzz comparisons for the intentionally different borrowing/auto-decoding
input surfaces. Exercise arbitrary encoded bytes, generated equivalent inputs,
every constructor, lifecycle, source spans and new limit categories. Extend
external infoset projection to supported UTF-16 rather than silently skipping it.
Keep unsupported DTDs/legacy encodings explicit. Update docs and raw coverage,
run all targets/doctests/fmt/strict Clippy, and complete an independent mutation
audit before integrating into local main.

UTF-16 output, other encodings, complete XML conformance, matched performance,
Rust 1.100 allocator support and 100% fuzz execution coverage remain open goal
items. Retained UTF-16 declarations require an explicit encoding change before
being copied to today's UTF-8-only writer.
