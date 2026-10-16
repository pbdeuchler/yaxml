# Matched DOM and streaming baseline

The P0 baseline must compare complete parsing work before DTD implementation
changes name lifetimes, entity input storage or tree layout. The target remains
better performance than libxml2 across the full workload matrix. This first
baseline measures the implemented UTF-8, namespace-aware, DTD-free profile;
unsupported features must remain visible in separate conformance results.

## Reference and ownership

Use the existing checksum-pinned libxml2 2.15.4 static Release build. Follow the
fresh-process pairing and bounded capture of `structure_benchmark`, rather than
timing process startup or xmllint's reused parser context. Cargo never compiles
or links the C helper. Core Rust, dependencies, parser limits and fuzz targets
remain unchanged.

| yaxml mode | libxml2 counterpart | Included parsing work |
| --- | --- | --- |
| `dom` | `xmlReadMemory` | Fresh borrowed DOM; reference DOM owns its strings. |
| `owned` | `xmlReadMemory` | Fresh source-independent logical tree on both sides. |
| `reader` | `xmlReaderForMemory` | Drain all events and inspect values. The reference copies input. |
| `push` | SAX2 push parser | Feed literal equal-sized chunks, finish and observe callbacks; no DOM. |
| `buffered` | `xmlReaderForIO` | Bounded buffer/read-callback over the same preloaded bytes, drain EOF. |

Input loading and fixture generation happen before all timers. Every iteration
constructs a fresh parser. DOM parse, public traversal and destruction have
separate summed clocks. Streaming construction, observation, consumption and
EOF checks share the parse clock; destruction is separate. The primary DOM
parse/destruction result is `parse_ns + drop_ns`, without traversal. Streaming
includes observation and therefore cannot be compared directly to DOM parsing.
Clock calls occur per iteration and matter on tiny documents.

Libxml2 uses `NONET | NO_XXE`, without recovery, DTD loading, entity substitution,
blank removal, `HUGE` or `COMPACT`. Its resource policies differ from yaxml's
finite defaults. This baseline requires successful work below both policies;
it does not claim equivalent rejection limits or hostile-input cost. It reports
copying, representation and frontend differences instead of hiding them.

## Observation protocol

`parse_bench` and `libxml2_parse_bench` accept `MODE INPUT ITERATIONS CHUNK`.
Each loads a regular input file of at most 8 MiB before timing, accepts 1–10,000
iterations and 1–65,536-byte chunks, and caps total source work at 512 MiB.
`--version` checks the exact engine/protocol and pinned reference version.
Failure produces a nonzero exit and no sample row.

One TSV row begins `YAXML-PARSE-BENCH-1`, followed by ordered `key=value` fields:
`engine`, `mode`, `bytes`, `iterations`, `chunk`, `parse_ns`, `observe_ns`,
`drop_ns`, `elements`, `attributes`, `namespaces`, `text_bytes`, `comments`,
`pis`, `hash`. Counts and hash describe one document; times sum all iterations.
Every iteration must produce the same observation. Streaming `observe_ns` is
zero because that work belongs to `parse_ns`.

The 64-bit FNV-1a digest observes a canonical logical transcript. A field is its
UTF-8 byte length as a little-endian u64 followed by all bytes. Start elements
emit `S`, qualified name and namespace URI fields, then each namespace
declaration (`N`, prefix, URI), then each ordinary attribute (`A`, qualified
name, URI, normalized value), then `>`. Declaration and ordinary-attribute
orders are preserved separately. End elements emit `E`, qualified name and URI;
empty elements emit both. Missing prefix or URI is an empty field.

Adjacent text and CDATA payloads form one `T` followed by every logical byte,
terminated with NUL before the next nontext observation or EOF. XML cannot
contain NUL, so arbitrary frontend fragmentation does not change framing.
Comments emit `C` and their value field; PIs emit `P`, target and data fields.
Declaration metadata, source spans, CDATA boundaries, empty-element spelling
and top-level whitespace are excluded. This is a noncryptographic consistency
check, not canonical XML or a replacement for independent semantic tests.

## Driver and evidence gates

The driver creates a private copy of the input, checks its bytes before and
after every helper invocation, and requires strict version and result schemas.
One unrecorded pair validates observations. Retained pairs alternate execution
order in fresh processes; each must match both the other implementation and
the preflight observation before publication. A later failure leaves a nonzero
command result even if earlier successful pairs were printed.

The existing subprocess runner bounds each child to 60 seconds by default and
combined output to 4 KiB. `--timeout-ms` can reduce the deadline to 1–60,000 ms.
It trusts the caller-selected executable. These are not OS
memory/disk quotas, filesystem deadlines or a hostile-filesystem sandbox.

The versioned corpus generator includes tiny, flat, long-text, attribute-heavy,
namespace-heavy, normalized, Unicode, deep and mixed-content cases. Retain exact
input hashes, source/binary identities, compiler/reference options, CPU/OS,
sample order, iterations and every raw row. Report medians and spread for every
case and mode. Byte/chunk size series, latency, allocations, retained and peak
memory, malformed inputs, other encodings and unsupported features remain
separate required gates. No average may conceal an individual regression.

The version handshake verifies protocol, engine and reference-version
compatibility. It intentionally permits debug helpers for functional tests;
it does not establish optimization or source identity. Retained performance
evidence requires a frozen outer recipe that verifies committed source blobs,
records the complete release build commands and compiler/reference settings,
and compares source and binary hashes before and after the entire run. A
mismatch invalidates publication of that run.

Acceptance requires actual-command happy and error tests, independent C/Rust
observations across chunk sizes, restored-fault tests of schema/comparison/exit
gates, strict builds and a reviewed immutable evidence checkpoint. Full
performance superiority remains unproven until the wider matrix is measured.
