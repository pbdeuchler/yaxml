# Independent libxml2 oracle protocol

The compatibility target is libxml2 **2.15.4**, compiled version `21504`.
The official [source archive](https://download.gnome.org/sources/libxml2/2.15/libxml2-2.15.4.tar.xz)
has SHA-256 `98087fd181d9070724f3fbc65c7377db03038eb92bd882374daff44940138821`,
published in GNOME's [checksum file](https://download.gnome.org/sources/libxml2/2.15/libxml2-2.15.4.sha256sum).
External oracle tooling adds no dependency or unsafe code to the yaxml library.

`examples/support/infoset.rs` builds an XPath 1.0 query and expected stdout from
the yaxml event reader for UTF-8 or owned-tree views for UTF-16. Run the query against the **original input** with:

```text
xmllint --nonet --nocatalogs --strict-namespace --nowarning --nocdata --xpath QUERY -
```

Compare stdout byte for byte against `Projection::expected()`. The oracle's
strings are never reparsed with yaxml, so a shared namespace or normalization
bug cannot make both sides agree merely by traversing the same Rust parser.
The CLI caller must check oracle version, successful exit, process time and
output ceilings separately. Failure to build a bounded projection is an
explicit skipped semantic comparison, never semantic agreement. The replay CLI
still checks acceptance in that case and counts it separately.

After the original input agrees, the replay CLI serializes the complete tree
and runs the same query against the output, using the original expected values.
UTF-8 uses `write_document`; UTF-16 uses `write_owned_document`. The output is
UTF-8 and bounded by `9 * original_input.len() + 1024`, allowing UTF-16 decoding
and XML escaping. Each subprocess retains the same time/output limits. An
owning-parser or writer OwnedBytes ceiling is separately counted as acceptance
only (`serialized storage bytes`), never full semantic agreement. Other
serialization failures fail the replay. A semantic result therefore requires
both the original and serialized bytes to agree with the projection. This
comparison inherits every omission listed below; the strict fuzz tree oracle
separately checks declarations, ordered XMLNS attributes and empty lexical nodes.

The first field is libxml2's actual normalized descendant node count. For each
node in document order, subsequent fields contain kind, depth, qualified name,
local name, namespace URI, value and actual attribute count. Attribute fields
contain qualified name, local name, namespace URI and normalized value, in
source attribute order. Kind codes are element `1`, text `2`, comment `3`, and
processing instruction `4`; depth counts the synthetic document ancestor, so
the root element has depth `1`. Element value fields are empty: descendant text
already appears in its own nodes. PI qualified and local names both contain
the target. Namespace declaration attributes are excluded from attribute counts.

Each string is framed as its decimal Unicode scalar count, a colon, and the
complete value. XPath `string-length` matches Rust `str::chars().count()` for
accepted XML characters. Framing preserves embedded colons, quotes, newlines,
CR, tabs and multibyte characters without an escaping decoder. The complete
stdout contains one final LF added by `--xpath`. Query strings contain only
fixed XPath and generated integer positions; XML names and values cannot
inject query syntax. Actual node and attribute counts catch extra records
beyond the expected positions.

Adjacent text and CDATA are merged, and empty text is omitted. `--nocdata`
coalesces nonempty text, but local libxml2 probes retained some empty CDATA as
empty text nodes. The XPath selector therefore explicitly excludes those nodes.
Positions are attached directly to the forward descendant axis. Local 2.15.3
probes showed that parenthesized node-set indexing can reorder an epilog comment
before a nested element; direct axis positions preserved document order.

The helper rejects more than 256 projected nodes, 4,096 ordinary attributes,
64 KiB of query text or 8 MiB of expected stdout. The query ceiling leaves room
under Linux's per-argument limit. Queries repeat node selections and are intended
for bounded conformance fixtures, not throughput measurements. Reader resource
and owned-parser limits apply independently. Owned-pool exhaustion while building
a UTF-16 projection is an explicit `projection storage bytes` ceiling and counts
as acceptance-only. A small node or attribute count can still exceed
the query or output ceiling.

This is a normalized infoset projection. It deliberately omits declaration
metadata, source spans, empty-tag spelling, CDATA boundaries, namespace
declaration placement and unused namespace bindings. Effective expanded names
are compared for every element and ordinary attribute. The namespace axis is
deferred: local libxml2 exposes an empty-prefix, empty-URI namespace node after
default namespace undeclaration. UTF-16 values are compared directly from owned
views against the oracle reading original bytes. DTDs and unsupported encodings remain outside
the initial supported profile. Comparing source attribute order is stricter than
an unordered attribute infoset and is an explicit protocol choice.

Declared-encoding mismatches are counted as policy skips because yaxml rejects
them strictly while libxml2 can warn and retain its detected encoding. This
documented difference does not count as compatibility agreement. Malformed
UTF-16 code units still participate in rejection comparisons.

The alternatives have weaker contracts. Pinned [xmllint source](https://raw.githubusercontent.com/GNOME/libxml2/v2.15.4/xmllint.c)
shows that SAX debug output truncates text, CDATA and attribute values. Local
tree debug output also truncated long content and rewrote whitespace; streaming
debug output omitted attributes and namespace identity and emitted unescaped
values. Ordinary UTF-8 serialization preserves more information, but reparsing
it with yaxml shares semantic blind spots. C14N folds CDATA, removes declaration
metadata, changes attribute order, and rejected a relative namespace URI in a
local probe. Debug output is documented as unstable in the [xmllint manual](https://gnome.pages.gitlab.gnome.org/libxml2/xmllint.html).

Exploratory checks used Homebrew libxml2 **2.15.3**, not the pinned oracle. Eleven
fixtures agreed exactly, including long values, Unicode, CR/LF/tab references,
all supported node kinds, namespace rebinding, relative namespace URIs and empty
CDATA. A matching 2.15.4 execution remains required before recording a pinned
semantic result.

## Reproducible runner and build

The replay example performs version, feature and process checks:

```sh
cargo run --release --example fuzz_replay -- --libxml2 /path/to/2.15.4/xmllint fuzz/corpus/*
```

For investigation with another installation, add `--allow-other-libxml2`. The
runner labels this exploratory and continues to require Tree/XPath support.
Unexpected exits or diagnostics fail the comparison. Forward-version warnings
are suppressed, while strict namespace failures still produce rejection.

The `libxml2-oracle` CI job verifies the official archive checksum before
extraction. It builds static Release `xmllint` with CMake, disables external
Iconv/ICU/Zlib and Python dependencies, and enables XPath/output. Documentation
and upstream tests are not built. Remaining feature defaults come from the
pinned [CMake configuration](https://github.com/GNOME/libxml2/blob/v2.15.4/CMakeLists.txt).
Artifacts include the runtime version/features, full CMake option report,
comparison summary, Rust compiler, revision and corpus hashes. Hosted execution
has not yet been observed; a locally installed 2.15.3 is an exploratory oracle.

Subprocesses receive input and capture output through private temporary files.
The runner checks a five-second child-execution deadline and combined output
ceilings, then performs bounded final reads. Both catalog environment variables
are explicitly empty. The executable is caller-selected and trusted: this is
not a sandbox. Disk writes can overshoot between polls; file operations have no
deadline; termination targets the direct child; temporary cleanup is best effort.
These limits are distinct from yaxml's parser budgets.
