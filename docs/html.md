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
| `html::parse_events(bytes, Options, handler) -> Result<Summary, Error>` | `htmlNewSAXParserCtxt` + `htmlCtxtReadMemory` with custom SAX callbacks |
| `html::EventParser::{new, feed, finish}` | `htmlCreatePushParserCtxt` with custom SAX callbacks |
| `html::parse_fragment(bytes, encoding, Options)` | `xmlParseInNodeContext` / `xmlCtxtParseContent` on a node of an HTML document |
| `Diagnostic::{describe, render}` with `LineIndex::html(input, &document)` or `Summary::line_index(input)` | `xmlError` (`describe`), `xmlFormatError`'s report as `xmllint --html` prints it (`render`) |
| `OwnedNode::line(&LineIndex::html(input, &document))` on an HTML node | `xmlGetLineNo` |
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
| `HTML5` | report tags as read (below) |
| `RECOVER`, `NOWARNING`, `PEDANTIC`, `NONET`, `COMPACT`, `BIG_LINES`, `XML_PARSE_NOENT` | accepted; no effect here (or in libxml2; lines are never capped at 65535) |
| any other bit | rejected with `ErrorKind::UnsupportedOption` |

## Events

`parse_events` and `EventParser` run the same tokenizer and tree construction
without a tree and hand each SAX callback libxml2 makes to a closure as an
`Event`: `Doctype` (`internalSubset`, for every DOCTYPE wherever it appears),
`Start { name, attributes }` (`startElement`, implied elements included;
valueless attributes have `None`, where the tree builder gives boolean ones
their name), `End` (`endElement`), `Text` (`characters`), `Cdata`
(`cdataBlock`, raw text) and `Comment`. The variant names follow
`reader::Event`. Text arrives in libxml2's pieces (`t`, `&`, `u` for
`t&amp;u`), so consecutive `Text` events may continue one another; the pieces
do not depend on how push input is split. Returning `ControlFlow::Break`
stops parsing like `xmlStopParser`: no further events, `Summary::stopped`.

What libxml2's SAX path does differently from its tree, these events do too:
text outside any element is delivered (the tree builder drops it), a
discarded start tag with no current element leaves its `>` to be read as text,
there is no depth ceiling (`nodePush` enforces it in the tree builder), and
`NOBLANKS` drops ignorable blanks without an event (it installs a no-op
`ignorableWhitespace`), keeping the blanks `areBlanks` can only judge with a
tree. `Summary::diagnostics` equal the tree's when neither stopped.

## HTML5 mode

In 2.15.4 `HTML_PARSE_HTML5` does not select a different tokenizer (the one
tokenizer already follows the WHATWG tokenization rules) and adds no tree
builder: it turns tree construction off. Every start tag calls `startElement`,
every end tag `endElement`, with no implied elements, auto-closing,
misplaced-tag checks, end-tag matching or closing at EOF; void and
self-closing elements get no `endElement`; DOCTYPE names are lowercased. The
name stack keeps only the last start tag, for raw-text end tags and
`areBlanks`. That is the mode libxml2 itself uses for html5lib's tokenizer
tests (runtest.c); `tests/internal/html_events.rs` replays all 6,972 records
of `test/html-tokenizer`, in every tokenizer state, against
`result/html-tokenizer` and all match. A tree built in this mode follows the
unbalanced calls as libxml2's SAX2 builder does: an end tag closes the
current element whatever its name, and text without a current element is
dropped.

## Fragments

`parse_fragment` parses content as `xmlParseInNodeContext` does for an HTML
document: below a `#root` element on the name stack, with no implied
elements (libxml2 forces `NOIMPLIED`), no blank skipping and no DOCTYPE (a
DOCTYPE is not kept outside the prolog). The context node does not change
HTML parsing, so it is not a parameter; the input is read in the context
document's encoding (pass `OwnedDocument::html_encoding`), and without one it
is sniffed from `<meta>` or the first non-ASCII byte but never from a
byte-order mark, as libxml2 does. The nodes are the top-level children of the
returned document (text included), to be copied with `Editor::import`.
libxml2 returns the last error code for an empty list; the diagnostics carry
it here.

## Lines and diagnostics

Lines cost nothing while parsing: as for XML, positions are recorded and
lines are counted from the input only when asked for. Every parsed element,
text and comment has as its origin (`OwnedNode::range`) the empty range, in
the decoded text, where libxml2 takes its line when the SAX2 builder creates
it: an element at its start tag's `>` (an implied one at the token that
implied it), a comment where it ends, and text where its last
character-data run began, because libxml2 updates its line only after a run
and moves a merged text node's line forward. `LineIndex::html(input,
&document)` (or `Summary::line_index(input)` for events) re-decodes the input
the way the parser did, from a record of the decoding the document keeps (a
skipped BOM, the first converter, and at most one switch at a text offset,
for a `<meta>` charset or the windows-1252/prescan guess); `OwnedNode::line`
then returns `xmlGetLineNo`'s line (other nodes take their previous
sibling's or parent element's line). Lines are never capped at 65535;
constructed nodes have none. For BOM-less UTF-8 or ASCII input
`LineIndex::new(input)` gives the same lines.

Each `Diagnostic` keeps libxml2's message, its `offset` in the decoded text
(where libxml2's input is when it raises the error: after an end tag, after a
misplaced tag's name, at a `<meta>`'s `>`, at the start of the scan that met
invalid bytes) and the offset libxml2 takes its line from. With the index,
`describe(&lines)` gives the `xmlError` (domain HTML, or parser for the depth
ceiling, I/O for a converter error; code, level, message, line, a column
counted in characters) and `render(file, &lines)` the report `xmllint --html`
prints, context line and caret included, as `yaxmllint --html` now does. The
one state recorded while parsing is whether an error came before a charset
switch: its context line then shows the text as it was, before re-decoding.
As in libxml2 at most 100 errors are reported, resource limits aside.

Parse cost, in retired instructions for 10 parses (median of 3) and best of
40 wall-clock parses of generated 4.6-7.9 MB documents, against the
pre-branch base (0576991): small elements 33.70G → 26.79G instructions,
236 → 204 ms; attribute-heavy markup 16.55G → 14.86G, 115 → 107 ms; text
3.61G → 3.10G, 26.3 → 23.6 ms. Most of that is an unrelated fix in the
same pass: end tags looked up the eleven priority elements in a SipHash map
(up to thirteen hashes per element); they are now an array. Against the base
with that fix alone, the SAX sink, positions and decoding record cost 1.3%,
0.5% and 1.0% of instructions.

## Encodings

As in libxml2: a UTF-8 or UTF-16 BOM, or `<?` in UTF-16, selects the encoding
(fewer than four bytes are never sniffed); input starting with `<?xm` is UTF-8;
otherwise a `<meta charset>` or `http-equiv=Content-Type` declaration seen
before any non-ASCII text switches the encoding, and the first non-ASCII byte
in text or a value runs libxml2's prescan (`HTTP-EQUIV` ... `CONTENT` ...
`CHARSET=`, up to the first NUL) and otherwise falls back to windows-1252.
Tag names before that point are read as UTF-8. Invalid UTF-8 bytes become
U+FFFD one byte at a time, as `htmlValidateUtf8` does.

Multi-byte converters are not implemented, deliberately: the pinned oracle is
built without iconv, ICU and ISO8859X, and libxml2's `defaultHandlers` then
have no converter for UCS-4, UCS-2, IBM037, ISO-2022-JP, Shift_JIS or EUC-JP,
and no handler at all for GBK, Big5, EUC-KR and the like. `htmlParse` on such a
`<meta charset>` reports "Unsupported encoding: shift_jis" and falls back to
windows-1252 at the first non-ASCII byte, which this module matches (corpus
cases for each label). The owner wants iconv-only encodings at low priority
([bindings.md](bindings.md#decisions-owner-2026-10-19)); they belong with
caller-supplied converters (roadmap item 9), with a differential against an
iconv-enabled libxml2 build rather than this oracle.

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

`matches_xmllint` compares the `xmllint --debug` tree, the serialization and
stderr (every rendered report) for the snippets in
`tests/support/html_corpus.rs` (structure, auto-close, end tags, references,
raw text, comments, DOCTYPE, attributes, encodings, whitespace and
html5lib-style tokenizer inputs) plus, with the source tree, libxml2's
`test/HTML` documents and the data-state inputs of `test/html-tokenizer`:
7,044 inputs in five modes (default, `--format`, `--push`, `--noblanks`,
`--nodefdtd`).

xmllint has no HTML5 flag and prints neither SAX callbacks verbatim nor
lines, so `matches_libxml2_callbacks_lines_and_fragments` drives libxml2
directly through `yaxml-harness/tooling/libxml2_html.c`:

```sh
ORACLE=/path/to/yaxml-oracle
cc -std=c11 -Wall -Wextra -Werror -I$ORACLE/build \
  -I$ORACLE/libxml2-2.15.4/include tooling/libxml2_html.c \
  $ORACLE/build/libxml2.a -lm -lz -liconv -o target/html-helper
YAXML_HTML_HELPER=$PWD/target/html-helper YAXML_LIBXML2_SOURCE=... \
  cargo test --test html_oracle -- --nocapture
```

It compares, for the same 7,044 inputs, every callback with its exact text
pieces and every error (domain, code, level, line, message) under options 0,
`NOBLANKS`, `NOIMPLIED` and `HTML5`; every node's `xmlGetLineNo` with options
0 and `HTML5`; and `xmlParseInNodeContext` into a `<body>` (result code, node
list, errors).

| Mode | Before (2026-10-19, 0576991) | After |
| --- | --- | --- |
| xmllint tree, output, error count (5 modes) | 7,044 / 7,044 each | 7,044 / 7,044 each |
| xmllint stderr reports (5 modes) | 0 (not rendered) | 7,044 / 7,044 each, except `--push`: 7,043 (`doc3.htm`, where xmllint's own push and pull reports differ) |
| SAX callbacks: default, NOBLANKS, NOIMPLIED, HTML5 | not implemented | 7,044 / 7,044 each |
| node lines: default, HTML5 | not implemented | 7,044 / 7,044 each |
| fragments | not implemented | 7,044 / 7,044 |
| html-tokenizer results (all states, HTML5) | not implemented | 6,972 / 6,972 |
| error columns (`xmlError.int2`) | none | 2 / 87 (we count characters) |

## Fuzzing

`tests/support/fuzz/html.rs` derives options, small limits and a push chunk schedule from
each input and checks that push equals pull, tree invariants and that saved
output reparses (see [yaxml-harness/fuzz/README.md](https://github.com/pbdeuchler/yaxml-harness/blob/main/fuzz/README.md)). Seeded with the
corpus snippets and libxml2's `test/HTML` files,
`cargo +nightly fuzz run html yaxml-harness/fuzz/corpus/html yaxml-harness/fuzz/corpus -- -max_total_time=360 -max_len=16384`
ran 302,479 inputs in 6 minutes without failure. Its smoke tests had found two
chunk-dependence bugs, and an earlier run an over-strict reparse invariant
(`yaxml-harness/fuzz/regressions/html/reparse-prescan.html`). `yaxml-harness/coverage.sh html` on the
resulting corpus covers 99.8% of tokenizer lines (98.7% of branches), 98.8%
of tree lines (92.9%), 94.2% of serializer lines (92.3%), 94.9% of encoding
lines (93.6%) and 98.3% of the driver (90.4%).

Targets `html_events` (events: push equals pull, the SAX2 replay of the
events equals `parse`'s tree and diagnostics, `Break` ends them) and
`html_fragment` cover the new entry points; `HTML5` is one of the derived
options in all three. On 2026-10-20, 10 minutes each on nightly-2026-10-18
(`-fork=1 -timeout=2 -max_len=16384`, seeded with `yaxml-harness/fuzz/corpus/html`):
`html` 134,586 inputs, `html_events` 504,791, `html_fragment` 739,904, no
failures; after lines became lazy, another 10 minutes each (seeded with
those corpora): 199,919, 423,770 and 710,571 inputs, no failures. A first `html` run found an over-strict invariant: an HTML5 tree
can have several top-level `html` elements, and libxml2 (and this
serializer) gives only the first head a generated `<meta charset>` when that
head is empty, so per-subtree output differs
(`yaxml-harness/fuzz/regressions/html/html5-subtrees.html`).

## Known differences

- libxml2's push parser delivers text at chunk boundaries, so its `NOBLANKS`
  decisions and prescan can depend on chunking; ours equal pull parsing.
  `xmllint` reads files in chunks, so its prescan can see less than the rest
  of the input.
- On a value longer than its ceiling libxml2 still creates the element with
  the attributes parsed so far; this module drops the tag and stops. The
  default ceiling is 1 MiB (libxml2: 10,000,000 bytes).
- Malformed UTF-16 becomes U+FFFD; libxml2 reports a conversion error.
- `Diagnostic::column` counts characters from the line's start. libxml2's
  `xmlError.int2` skips tag and attribute names and counts the byte that ends
  a text run or value twice; xmllint does not print it.
- A converter error (malformed UTF-16, an unmapped single-byte value) is
  libxml2's I/O error, reported at the token holding the first malformed
  character (libxml2: wherever the parser is when it converts that buffer,
  which depends on its buffering), and parsing continues with U+FFFD
  instead of stopping.
- Push and pull give the same context lines (they are computed from the
  input afterwards); libxml2's push contexts depend on its chunking.
- Warnings for unusable prescan labels are not reported.
- A DOCTYPE in a fragment is dropped; libxml2 replaces the context document's
  DOCTYPE only in its prolog state, which content parsing never is.
- `htmlCtxtReset` and node-info recording (`HTML_PARSE_*` record_info) are
  not implemented.
