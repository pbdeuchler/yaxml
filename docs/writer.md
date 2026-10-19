# UTF-8 event writer

`Writer<W: std::io::Write>` serializes the existing `Event` types to an explicit
sink. It validates each complete encoded event before writing any of that event's
bytes. Its shared parser state checks document structure, declarations, names,
namespace scope, duplicate attributes, and resource limits.

Events from `Reader` already include the namespace metadata the writer requires:

```rust
use yaxml::{Reader, Writer};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let input = br#"<p:root xmlns:p="urn:example" key="a&amp;b"/>"#;
    let mut reader = Reader::new(input)?;
    let mut writer = Writer::new(Vec::new());
    while let Some(event) = reader.read_event()? {
        writer.write_event(&event)?;
    }
    writer.finish()?;
    assert_eq!(writer.into_inner().as_slice(), input);
    Ok(())
}
```

`new(sink)` uses `WriterLimits::default()`. `with_limits(sink, limits)` selects
other inclusive ceilings. Neither constructor performs I/O. `write_event(&event)`
borrows the supplied event for the call; retained names and namespace bindings
are copied, so later calls do not depend on its storage.

`write_document(&Document)` and `write_owned_document(&OwnedDocument)` require a
fresh writer. They traverse the complete tree without recursion, validate each
generated event through the same writer state, flush the sink, and leave the
writer finished. Any earlier event call consumes freshness, including empty
Text. A document-call failure is terminal and preserves the same partial-output,
position, panic, and I/O-error behavior as event writing.

Complete-tree output is compact UTF-8 with no BOM, indentation, inserted
separator, or trailing newline. Declaration absence, version, and standalone are
preserved; a present encoding is rewritten to `UTF-8`. Qualified and expanded
names, explicit namespace declarations, ordered attributes, document-child
order, and node kinds are preserved. Every childless element becomes `<name/>`
because the immutable DOM does not retain the source's empty-tag spelling.
Top-level whitespace was not retained and is not reconstructed.

## Namespaces and spelling

When constructing events directly, every `Name` must have matching `qualified`,
`prefix`, `local`, and `namespace` fields. The qualified spelling determines the
prefix and local name; the namespace must match the current declarations. An
empty namespace is `None`, rather than `Some("")`.

Declare namespaces through ordinary entries in `Element::attributes`, including
the XMLNS namespace metadata supplied by `Reader`. The XMLNS URI is
`http://www.w3.org/2000/xmlns/`. A declaration on an element applies to that
element and its attributes regardless of attribute order. Default namespaces
apply to unprefixed elements; unprefixed ordinary attributes have no namespace.
The predefined `xml` prefix resolves to `http://www.w3.org/XML/1998/namespace`.
Reserved-prefix restrictions, undeclared prefixes, and duplicate expanded
attribute names are rejected. Bindings are restored when their scope ends.

Start and end events produce explicit tags; `Empty` produces `<name/>`.
Attributes keep event order and use double quotes. Declarations keep their field
values and emit version, optional encoding, then optional standalone, without a
trailing newline. Only UTF-8 encoding declarations are supported. A declaration
is optional and can appear only at the start.

Text escapes `&`, `<`, and `>` as `&amp;`, `&lt;`, and `&gt;`; CR becomes `&#13;`.
Escaping every text `>` also prevents adjacent events from producing a raw
`]]>` delimiter. Attribute values escape `&`, `<`, and double quotes; TAB, LF,
and CR become `&#9;`, `&#10;`, and `&#13;`. Attribute `>` stays literal.
These references preserve the caller's logical text and attribute values.

Comments, CDATA, and PI data are written literally within their delimiters.
Comments containing `--` or ending in `-`, CDATA containing `]]>`, and PI data
containing `?>` are rejected. All generated text must contain valid XML
characters. A PI gets one separator space when its data is nonempty. On reparse,
XML line-ending normalization changes raw CR/CRLF in these three value kinds;
the reader also trims leading XML whitespace from PI data.

Only XML whitespace is allowed as text outside the root. It is written literally,
including CR, because character references are forbidden there. Empty Text emits
no bytes. Empty Text and top-level whitespace do not count as semantic events.
Adjacent Text events can coalesce into one lexical text run when reparsed;
CDATA and empty-tag spelling are preserved only when represented by those events.

## Limits and retained state

| `WriterLimits` field | Default | Counts |
| --- | --- | --- |
| `max_output_bytes` | 64 MiB (67,108,864 bytes) | All encoded bytes confirmed by the sink, including markup and references. A complete next event must fit the remaining budget before I/O. |
| `max_token_bytes` | 1 MiB (1,048,576 bytes) | Encoded bytes of one supplied event. |
| `max_depth` | 256 | Element nesting, with the root at depth one. |
| `max_attributes` | 1,024 | Attributes per element, including namespace declarations. |
| `max_events` | 1,000,000 | Shared semantic events, including declarations, Empty, and empty CDATA. Complete-tree output counts emitted events after folding childless elements to one Empty event. Empty Text and top-level whitespace are ignored. |
| `max_owned_bytes` | 64 MiB (67,108,864 bytes) | Cumulative requested string capacity for normalization, namespace copies, and retained structural names, including allocations already released. |

Encoded expansion counts against both byte ceilings. The attribute-count limit
is checked before serializing an attribute slice. Encoded scratch storage grows
fallibly up to the token ceiling, and its capacity can retain a high-water mark.
Other retained state depends on open elements and active/shadowed namespace
bindings. Inactive interned namespace strings are released when their last
binding leaves scope; metadata capacity can remain allocated.

The owned-byte budget is cumulative allocation work, not a resident-memory quota.
Scratch token storage is bounded separately. Caller-owned events and sink buffers
are outside this accounting. Writer and reader budget consumption can differ:
writer escaping adds bytes, validation may normalize references again, and
adjacent Text events can exceed the reader's single-token ceiling after merging.
Choose reparse limits for the encoded output rather than assuming equal quotas.

## Completion and failures

`finish()` requires exactly one completed root and then flushes the sink.
It reports `MissingRoot` or `UnexpectedEof` for an incomplete document and does
not generate missing end tags. Repeating successful finish performs no I/O.
Any later write, including empty Text, starts terminal `InvalidState`.
Successful complete-document calls already perform this finish operation;
subsequent `finish()` calls remain harmless and perform no I/O.

The first failure returns `WriteError::Xml(Error)` or the original
`WriteError::Io(std::io::Error)`. Display and `source()` preserve that cause.
Every later write or finish returns XML `InvalidState` without touching the
sink. A caught sink panic leaves the same terminal state. Interrupted write and
flush calls are retried; zero-byte writes produce `WriteZero`, and an oversized
reported byte count produces `InvalidData`.

Validation failure emits none of the rejected event, but earlier output remains.
I/O failure can leave part of the current event in the sink. `position()` counts
bytes confirmed by successful write returns, including those before a later
failure. Bytes accepted inside a panicking sink call cannot be measured. XML
error offsets refer to prospective encoded output and can exceed `position()`
even though the rejected event emitted no bytes. Existing output cannot be
rolled back; the caller decides how to discard or retain it.

`into_inner(self)` returns the sink without validating, flushing, or adding end
tags. The writer performs no I/O on drop. The sink's own destructor retains its
usual behavior, so dropping a `BufWriter` can still flush its buffer.

## libxml2 tree output (`save`)

`yaxml::save::to_vec(tree, &options)` reproduces libxml2 2.15.4's `xmlsave.c`.
`tree` is a document (`&Document`, `&OwnedDocument`) or any node of either DOM
(`Node`, `OwnedNode`). A document node is `xmlSaveDoc`: declaration, then each
top-level node and a newline. Another node is `xmlSaveTree`/`xmlNodeDump`, with
no declaration. Namespaces the subtree uses but does not declare are added to
its root in first-use order, as on an `xmlDocCopyNode` copy, so an element
subtree is a namespace-well-formed document. Output is built in memory; the
walk is iterative. Unlike `Writer`, `save` does not validate events and follows
libxml2's spelling exactly.

`Options { flags, encoding, indent }` carries libxml2's `xmlSaveOption` values:

| Flag | Behavior |
| --- | --- |
| `FORMAT` | Newline and indent each child of element-only content. An element with a Text or CDATA child (Text only under XHTML) is written as is, with its descendants. Comments and PIs are indented, CDATA and text are not. Indentation stops at 60 columns (`MAX_INDENT`). |
| `WSNONSIG` | Newlines and indentation inside tags: before each attribute (level + 2), before `>`/`/>`, and before PI data. Content is untouched. |
| `NO_DECL` | No XML declaration. |
| `NO_EMPTY` | `<a></a>` for childless elements. |
| `XHTML` / `NO_XHTML` | XHTML 1.0 Appendix C rules; they apply automatically when the DOCTYPE names an XHTML 1.0 public or system identifier, unless `NO_XHTML`. |
| `AS_XML` / `AS_HTML` | XML or HTML serializer for HTML documents; `AS_HTML` routes an owned document through `html::save`'s serializer with the output encoding. |
| `NO_INDENT` | `FORMAT` line breaks without indentation. |
| `EMPTY`, `INDENT` | Accepted: they only override libxml2 globals whose defaults yaxml uses. |

Other bits are `UnsupportedOption`. `indent` (`xmlSaveSetIndentString`) is 1
to 60 spaces or tabs, two spaces by default; libxml2 accepts any string, which
can make the output malformed, so other strings are `UnsupportedOption`.

The declaration repeats the version (1.0 without one) and `standalone`, and
names the encoding label as given. HTML documents saved as XML are standalone
and name their HTML encoding, as libxml2 records them. `encoding` selects the
output: UTF-8; UTF-16 (little-endian behind a BOM); UTF-16LE and UTF-16BE
(no BOM); US-ASCII, ISO-8859-1..16 and windows-1252 under any libxml2 alias,
reverse-mapped through the input tables. `None` uses a document's declared
encoding. With no encoding at all, text and attribute values escape non-ASCII
as `&#xH;` (`XML_ESCAPE_NON_ASCII`) and markup stays UTF-8. Characters an
encoding lacks become `&#N;` in text, attribute values, attribute defaults and
entity values. libxml2's converter writes the same reference inside names,
comments, PIs, CDATA and DOCTYPE identifiers, where it is markup or literal
text; `save` returns `InvalidCharacter` at the output offset instead. Unknown
labels and libxml2's "HTML" converter are `UnsupportedEncoding`.

Escaping follows `xmlEscapeTab`/`xmlEscapeTabAttr`: `&`, `<`, `>` everywhere,
CR as `&#13;`, other C0 controls as `&#xFFFD;`; attributes add `"`, TAB and LF.
`xmlns` declarations come before other attributes, and `xmlns:xml` is dropped.
CDATA containing `]]>` is split after its `]]`. A PI with empty data is
`<?t?>`; the DOMs do not record libxml2's `<?t ?>` spelling.

The DOCTYPE is regenerated from the internal subset, as `xmlDtdDumpOutput` dumps
the parsed DTD: notations first, then elements, one `<!ATTLIST>` per attribute,
entities, comments and PIs in declaration order, the first of duplicate
declarations only, content models in libxml2's binary-tree spelling (including
its normalizations: `(a*)` is `(a)*`, `(a | b?)+` is `(a | b)*`), attribute
defaults normalized and escaped, entity values as written. Internal parameter
entities are expanded; their declarations appear after them. A subset that
declares nothing is dropped, comments included. Differences: libxml2 orders
notations by hash (seeded per process); declarations from external parameter
entities, which `xmllint --noent` loads, are absent because the subset is
parsed again without a resolver; a subset that pass cannot read is written as
it was parsed. Edited owned trees are written as they are: libxml2's save path
does not reconcile namespaces either.

XHTML output adds `xmlns="http://www.w3.org/1999/xhtml"` to a namespace-free
`html`, writes `<br />` for the XHTML 1.0 EMPTY elements and `<p></p>` for
others, expands empty boolean attributes (`checked="checked"`), copies `name`
to `id` (C.8) and pairs `lang`/`xml:lang` (C.7), and adds a Content-Type
`<meta>` to a top-level `head` without one. Subtrees use XHTML rules only with
the `XHTML` flag; HTML subtrees need `AS_XML`.

`tests/save_oracle.rs` compares bytes with `xmllint --noent --dtdattr` (and the
same with `--format`, `--pretty 2` and `--encode`) over the W3C XMLTS valid
documents and libxml2's `test/*.xml` and `test/xhtml*` (517 documents both
parsers accept), and `xmllint --html --xmlout [--format]` over libxml2's
`test/HTML` (94 of 94 identical). Run it with `YAXML_XMLLINT`:

| Mode | Identical | Notation order | External PE | PI spelling | Parser differences | Unrepresentable markup |
| --- | --- | --- | --- | --- | --- | --- |
| default | 498 (96.32%) | 4 | 9 | 2 | 4 | |
| `--format` | 514/523 (98.28%) | 5 | 1 | 2 | 1 | |
| `--pretty 2` | 502 (97.10%) | 2 | 9 | | 4 | |
| UTF-16, UTF-16BE | 499 (96.52%) | 3 | 9 | 2 | 4 | |
| ISO-8859-1 | 474 (91.68%) | 5 | 9 | 2 | 4 | 23 |
| US-ASCII | 470 (90.91%) | 4 | 9 | 2 | 4 | 28 |
| windows-1252 | 476 (92.07%) | 3 | 9 | 2 | 4 | 23 |

`--format` is compared on the tree `--noblanks` leaves, as yaxml's XML parser
has no NOBLANKS option. Notation counts vary between runs with libxml2's hash
seed. "Parser differences" are documents whose trees differ (libxml2 turns a
`&#13;` entity into LF and keeps two spaces from an entity in an attribute;
XML 1.1 line ends). The pinned build has no ISO-8859-2..16 converters, so those
are covered by unit tests against the decoding tables. The `save` fuzz target
checks DOM agreement, that only markup an encoding lacks fails, and that
saving reparsed output reproduces it byte for byte (see `fuzz/README.md`).

## Remaining scope

The writer supports the reader's UTF-8 XML profile, explicit event output, and
complete borrowed/owned DOM output; `save` adds libxml2's tree output.
Namespace repair, raw XML injection, `Write`-sink and `xmlSaveTree` level
parameters, HTML subtree output, iconv/ICU encodings and libxml2's remaining
writer convenience APIs are pending. This is neither a canonicalizer nor a
byte-preserving rewrite of source spelling.

The pinned [libxml2 writer](https://raw.githubusercontent.com/GNOME/libxml2/v2.15.4/xmlwriter.c)
informs the state and escaping boundaries. Its EndDocument operation can close
open elements before flushing; this API requires callers to supply complete
events so omitted end tags remain visible as errors. The writer adds no unsafe
code or Cargo dependency.
