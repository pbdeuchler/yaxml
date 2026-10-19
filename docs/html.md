# HTML module

`yaxml::html` reimplements the HTML parser and serializer of libxml2 2.15.4
(`HTMLparser.c`, `HTMLtree.c`). Since 2.14 libxml2 tokenizes HTML following
the HTML5 tokenization rules but builds trees with its own legacy algorithm:
implied `html`/`head`/`body`, the `htmlStartClose` auto-close table, end-tag
priorities and no adoption agency. This module reproduces that behavior, not
the WHATWG tree builder.

## API

| yaxml | libxml2 |
| --- | --- |
| `html::parse(bytes, Options) -> Result<Parsed, Error>` | `htmlReadMemory` / `htmlCtxtReadMemory` |
| `html::PushParser::{new, feed, finish}` | `htmlCreatePushParserCtxt`, `htmlParseChunk` |
| `html::to_html(&OwnedDocument)` | `htmlDocDumpMemory` (formatted, `<meta>` untouched) |
| `html::save(&OwnedDocument, format)` | `xmlSaveDoc` on an HTML document, as `xmllint --html [--format]` prints |
| `Parsed { document, diagnostics, stopped }` | `ctxt->myDoc`, reported errors, `XML_ERR_RESOURCE_LIMIT` halt |
| `OwnedDocument::{is_html, doctype, html_encoding}` | `XML_DOC_HTML`, `doc->intSubset`, `doc->encoding` |
| `Writer::write_owned_document`, `c14n` on an HTML document | XML output; fails if names, characters, comments or several top-level elements are not XML |
| `OwnedNode::attribute_has_value(i)` | `attr->children != NULL` |
| `OwnedDocument::element_by_id` | `xmlGetID` with HTML's `xmlIsID`: valued `id` attributes, and `name` on `a` |

Documents are ordinary [`OwnedDocument`](owned-dom.md)s, so traversal, text
chunks and editing work unchanged. HTML names are stored verbatim (they may be
names XML rejects) without namespaces; script, style and other raw-text
content is CDATA, as in libxml2. The DOCTYPE is the shared `Doctype` view of a
`DocumentType` node that, as in libxml2, is always the document's first child,
including the implied HTML 4.0 Transitional one unless `NODEFDTD`; an empty
`name` is a `<!DOCTYPE>` without one, and `internal_subset` is always None. A valueless
attribute reads as `""`; valueless boolean attributes (`checked`, `selected`,
...) get their name as value, as `xmlSAX2HtmlAttribute` does. HTML nodes have no
source origins. A document without any element (empty input, comments only,
`NOIMPLIED`) returns the document node from `root_element()`.

`Options { flags, limits }` takes the `htmlParserOption` bits:

| Flag | Behavior |
| --- | --- |
| `NOBLANKS` | drop blank text libxml2's `areBlanks` calls ignorable |
| `NOIMPLIED` | no implied html/head/body |
| `NODEFDTD` | no default DOCTYPE |
| `NOERROR` | collect no diagnostics |
| `HUGE` | depth ceiling 2048 and value ceiling 1 GB |
| `IGNORE_ENC` | ignore `<meta>` encoding declarations |
| `RECOVER`, `NOWARNING`, `PEDANTIC`, `NONET`, `COMPACT`, `BIG_LINES`, `XML_PARSE_NOENT` | accepted; no effect here (or in libxml2) |
| `HTML5` and any other bit | rejected with `ErrorKind::UnsupportedOption` |

## Encodings

As in libxml2: a UTF-8 or UTF-16 BOM, or `<?` in UTF-16, selects the encoding
(fewer than four bytes are never sniffed); input starting with `<?xm` is UTF-8;
otherwise a `<meta charset>` or `http-equiv=Content-Type` declaration seen
before any non-ASCII text switches the encoding, and the first non-ASCII byte
in text or a value runs libxml2's prescan (`HTTP-EQUIV` ... `CONTENT` ...
`CHARSET=`, up to the first NUL) and otherwise falls back to windows-1252.
Tag names before that point are read as UTF-8. Invalid UTF-8 bytes become
U+FFFD one byte at a time, as `htmlValidateUtf8` does.

Implemented: UTF-8, UTF-16LE/BE and libxml2's built-in single-byte tables
(US-ASCII, ISO-8859-1..16, windows-1252, from [crate encodings](encoding.md)),
with the HTML rule that ASCII and Latin-1 labels decode as windows-1252.
Labels libxml2 cannot convert are reported as
`DiagnosticKind::UnsupportedEncoding` and ignored, as libxml2 does. The pinned
oracle is built without ISO8859X support, so it reports ISO-8859-2..16 labels
as unsupported where this module decodes them; the differential corpus avoids
those labels. `html_encoding()` is the name libxml2 records:
the declared label, the prescan label, `windows-1252`, `UTF-8`, or None for
BOM-detected and ASCII-only input. Output uses that encoding (UTF-8 or a
single-byte table, UTF-16 with a little-endian BOM for a prescan label such as
`utf-16`, which libxml2 accepts there) or, without one, ASCII with HTML 4 entity names;
unrepresentable characters become `&#N;`.

## Push parsing and resource limits

The tokenizer is a resumable state machine (content mode, raw-text end tag,
prolog phase): each step scans one complete token from the unconsumed text or
consumes nothing. Push results, including text node boundaries and
diagnostics, therefore equal `parse` for every chunking. An incomplete token is
rescanned only after the pending text doubles, so total work stays linear; a
first non-ASCII byte without a known encoding waits for EOF so the prescan
sees the rest of the input.

Tree construction keeps the stack positions of each open element name, so
end-tag priority checks, `htmlCheckImplied` and misplaced-tag checks take
constant time and each element is popped once. Parsing and serialization are
iterative. `Limits::max_depth` (default 256, libxml2's) and
`Limits::max_token_bytes` (attribute values, comments, DOCTYPE parts and merged
text nodes) stop parsing with `DiagnosticKind::ResourceLimit` and
`Parsed::stopped`, keeping the tree built so far as libxml2 does. Input bytes,
attributes per tag, nodes (`max_events`) and owned bytes fail with
`ErrorKind::LimitExceeded`. A 20 MB document parses in about 0.24 s, like
`xmllint --html --noout`; adversarial inputs (5 MB unterminated comments,
a million blocked end tags, a 5 MB numeric reference, 1-byte push chunks) stay
linear.

## Differential

```sh
YAXML_XMLLINT=/path/to/pinned/xmllint \
YAXML_LIBXML2_SOURCE=/path/to/libxml2-2.15.4 \
  cargo test --test html_oracle -- --nocapture
```

The test compares the `xmllint --debug` tree, the serialization and the
number of reported errors for 310
snippets in `tests/support/html_corpus.rs` (structure, auto-close, end tags,
references, raw text, comments, DOCTYPE, attributes, encodings, whitespace and
html5lib-style tokenizer inputs) plus, with the source tree, libxml2's
`test/HTML` documents and the data-state inputs of `test/html-tokenizer`:
7043 inputs in five modes (default, `--format`, `--push`, `--noblanks`,
`--nodefdtd`). At last run all 7043 inputs matched in every mode, error
counts included.

## Fuzzing

`fuzz/html.rs` derives options, small limits and a push chunk schedule from
each input and checks that push equals pull, tree invariants and that saved
output reparses (see [fuzz/README.md](../fuzz/README.md)). Seeded with the
corpus snippets and libxml2's `test/HTML` files,
`cargo +nightly fuzz run html fuzz/corpus/html fuzz/corpus -- -max_total_time=360 -max_len=16384`
ran 302,479 inputs in 6 minutes without failure. Its smoke tests had found two
chunk-dependence bugs, and an earlier run an over-strict reparse invariant
(`fuzz/regressions/html/reparse-prescan.html`). `fuzz/coverage.sh html` on the
resulting corpus covers 99.8% of tokenizer lines (98.7% of branches), 98.8%
of tree lines (92.9%), 94.2% of serializer lines (92.3%), 94.9% of encoding
lines (93.6%) and 98.3% of the driver (90.4%).

## Known differences

- libxml2's push parser delivers text at chunk boundaries, so its `NOBLANKS`
  decisions and prescan can depend on chunking; ours equal pull parsing.
  `xmllint` reads files in chunks, so its prescan can see less than the rest
  of the input.
- On a value longer than its ceiling libxml2 still creates the element with
  the attributes parsed so far; this module drops the tag and stops. The
  default ceiling is 1 MiB (libxml2: 10,000,000 bytes).
- Malformed UTF-16 becomes U+FFFD; libxml2 reports a conversion error.
- Diagnostics carry a kind and a decoded-text offset, not libxml2's messages,
  codes or line numbers; warnings for unusable prescan labels are not
  reported.
- `HTML_PARSE_HTML5`, SAX callbacks, `htmlCtxtParseContent`, line numbers and
  node-info recording are not implemented.
