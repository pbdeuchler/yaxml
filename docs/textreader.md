# TextReader: libxml2's `xmlreader.h` cursor

`TextReader<R: BufRead>` is the `xmlTextReader` node cursor that lxml
`iterparse`, `Nokogiri::XML::Reader` and similar bindings drive. It wraps a
[`BufferedReader`](streaming.md), so it reads UTF-8, UTF-16 and the built-in
single-byte encodings from `&[u8]` or any `BufRead` in bounded memory.

| libxml2 | `TextReader` |
| --- | --- |
| `xmlReaderForMemory`, `xmlReaderForIO` (with `options` and `URL`) | `with_options` ([`ParseOptions`](compatibility.md#option-compatibility), whose `base` is the URL), `new`, `with_limits`, `with_decode_limits` |
| `xmlTextReaderSetResourceLoader` | `ParseOptions::with_resolver(base, resolver)` |
| `Read`, `Next`, `ReadState` | `read`, `next`, `read_state` |
| `NodeType`, `Depth`, `IsEmptyElement`, `HasValue`, `HasAttributes`, `AttributeCount` | same names, snake case |
| `ConstName`, `ConstLocalName`, `ConstPrefix`, `ConstNamespaceUri`, `ConstValue`, `ConstXmlLang` | `name`, `local_name`, `prefix`, `namespace_uri`, `value`, `xml_lang` (borrowed `&str`) |
| `BaseUri`, `LookupNamespace` | `base_uri` (owned), `lookup_namespace` |
| `GetAttribute`, `GetAttributeNo`, `GetAttributeNs` | `get_attribute`, `get_attribute_no`, `get_attribute_ns` |
| `MoveToAttribute`, `…No`, `…Ns`, `MoveToFirstAttribute`, `MoveToNextAttribute`, `MoveToElement` | `move_to_*` returning `bool` |
| `ReadString`, `ReadInnerXml`, `ReadOuterXml`, `Expand` | `read_string`, `read_inner_xml`, `read_outer_xml`, `expand` (an `OwnedDocument`) |
| `ReadAttributeValue` | `read_attribute_value` |
| `Standalone`, `IsDefault`, `Normalization` | `standalone` (`Standalone`), `is_default`, `normalization` |
| `ByteConsumed`, `GetRemainder` | `byte_consumed`, `into_remainder` |
| `GetParserProp`, `SetParserProp` | `parser_property`, `set_parser_property` |
| `XML_PARSE_DTDVALID`, `SetParserProp(VALIDATE)` | `DTDVALID` in `with_options`, `set_parser_property(Validate, true)`, `dtd_diagnostics` |
| `RelaxNGSetSchema`, `SetSchema`, `IsValid` (`RelaxNGValidate` and `SchemaValidate` are `Schema::parse` plus `SetSchema`) | `relaxng_set_schema`, `xsd_set_schema`, `is_valid`, `relaxng_diagnostics`, `xsd_diagnostics` |

`NodeType`, `ReadState`, `ParserProperty` and `Standalone` carry libxml2's
integer values (`as i32`). The reader produces Element, Attribute, Text, CDATA,
EntityReference, PI, Comment, DocumentType, SignificantWhitespace and
EndElement nodes, and so does libxml2's: `xmllint --stream --debug` over its
whole `test` tree (default, `NOENT`, `NOBLANKS`, `DTDVALID`) reports no other
type. `Entity` (6), `Document` (9), `DocumentFragment` (11), `Notation` (12),
`Whitespace` (13), `EndEntity` (16) and `XmlDeclaration` (17) exist for their
values only. Why each is unreachable in 2.15.4: entities and notations are not
tree nodes (declarations are `XML_ENTITY_DECL`, reported as type 0 only if the
reader entered the DTD, which it does not); the cursor starts at the root
element; the XML declaration is not a node; `xmlTextReaderNodeType` answers
`Whitespace` only when `xmlNodeGetSpacePreserve` returns 0, but that function
returns -1 for a text node, so every blank text is `SignificantWhitespace`,
under `xml:space="default"` and in DTD element content too.

## Semantics

The reader behaves like `xmlReaderForMemory(..., options)`. By default (no
flags) an entity reference is one EntityReference node (type 5) named after
the entity, with no value, whose content is not visited, and no DTD default
attributes appear; with `NOENT` the replacement's nodes appear in its place
and with `DTDATTR` the defaults. `NOBLANKS` drops ignorable blanks (below) and
`DTDVALID` validates against the DTD ([below](#dtd-validation)). `XINCLUDE` is
`UnsupportedOption`. `RECOVER` recovers as the streams and DOMs do
([streaming.md](streaming.md#recovery-and-blanks)), with the recovered errors in
`errors()`; libxml2's reader instead fails at its first well-formedness error
(`xmlTextReaderPushData` returns -1 once `wellFormed` is 0, `RECOVER` or not), so
`xmllint --recover --stream --debug` agrees on 22.22% of `test/errors` and 36.11%
of XMLTS not-wf. Following libxml2:

- `<a/>` is one Element with `is_empty_element`; `<a></a>` has an EndElement.
- Adjacent text, including substituted replacement text, is one node; text
  on either side of a kept reference is two. Text made only
  of space, tab, CR and LF is SignificantWhitespace, whatever `xml:space` says
  (libxml2 asks `xmlNodeGetSpacePreserve` about the text node itself).
- The XML declaration and whitespace outside the root are not nodes.
- Namespace declarations are attributes, listed before ordinary attributes;
  their namespace is `http://www.w3.org/2000/xmlns/`, `xmlns:p` has prefix
  `xmlns` and local name `p`.
- An attribute's depth is its element's plus one. While on an attribute,
  `attribute_count` is 0 and the `get_attribute*` lookups return `None`.
  `get_attribute("p:x")` resolves `p` in scope; `move_to_attribute` matches
  the prefix lexically. A failed `move_to_attribute_no` returns to the element.
- `has_attributes` stays true on the EndElement of an element with attributes.
- `base_uri` combines `xml:base` values inward-out with `uri::resolve`, stops at
  `http://`, `ftp://` or `urn:`, then resolves against the document URI.
- `read_attribute_value` walks an attribute's value: its children are Text
  nodes (named `#text`, depth + 2, the text as value) and, without `NOENT`,
  EntityReference nodes for declared general entities (named after the entity,
  no value), as the attribute node's children in libxml2. A value with no
  reference, empty or not, is one Text, and so is a namespace declaration's URI.
  At the last child it returns false and stays there. From a child
  `move_to_next_attribute` is false (libxml2 compares the child's type),
  `get_attribute*` find nothing, `read_string` is the text, and
  `move_to_first_attribute`, `move_to_attribute*` and `move_to_element` leave
  it. Outer XML writes the kept references as `&name;` in attribute values
  (`expand`, which has no DTD, writes their text). The forms come from
  `Budget::kept` (see `dtd_tag::kept_form`), so DTD defaults with references
  have them too.
- `standalone` is `Standalone::NoDeclaration` (-1) before the first read and
  without an XML declaration, `Unspecified` (-2) for a declaration without
  `standalone`, else `No`/`Yes`.
- `is_default` is false and `normalization` true for every node: libxml2
  2.15.4's `xmlTextReaderIsDefault` is `return 0` (defaulted attributes
  included, checked in the differential below) and `xmlTextReaderNormalization`
  is `return 1`.
- `byte_consumed` is the input the parser has taken, `BufferedReader::position`:
  exact, lookahead of the current node included. libxml2 counts whole 4 KiB
  chunks pushed to its parser, so the two agree at the end of a document (its
  length; checked in the differential) and not before.
- `into_remainder` consumes the reader and returns the source (`None` when
  there is no current node, as libxml2 returns NULL), positioned after the
  bytes the parser took. The parser takes whole windows of a `BufRead` (as
  libxml2 does 4 KiB), so for a slice, one window, nothing is left; read from a
  `BufReader` with a small capacity to resume mid-stream.
- `read_string` is the text of a Text/CDATA node, or the concatenated
  descendant text of an element with children (`None` for an empty element).
- Inner/outer XML follow `xmlNodeDump` of a copy: childless elements are
  `<a/>`; text escapes `<`, `>`, `&` and CR; attributes also escape `"`, tab and
  LF; namespaces used inside the copy but declared outside it are added to the
  copy's root after its own declarations, in first-use order. Inner XML
  serializes each child as its own copy.
- `expand` parses the outer XML into an `OwnedDocument` under the reader's
  limits; kept references are written as their replacement there, since
  the copy has no DTD (outer XML writes `&name;`). The cursor does not move: later reads still visit the subtree.

`NOBLANKS` is libxml2's `areBlanks` as the reader's push parser applies it:
a blank-only text node is dropped before it is queued, so `read_string`,
inner/outer XML and `expand` see the tree libxml2 would hold. Never under
`xml:space="preserve"` (`default` re-enables it); in an element the DTD declares
with element content always; otherwise only if the blank is not the only child,
neither the first nor the previous kept child is text, and no reference follows
it; and once an element without `xml:space` has kept text from the document, it
keeps every later blank (libxml2's `*ctxt->space = -2`; the DOM `blanks` has
the same rule, which it lacked). Replacement text under `NOENT` is never
dropped, and does not count as text. Differences: libxml2 judges an entity's
replacement text in a context of its own (a blank at its start before an
element is dropped, at its end kept; `ent2` and `ent10` of the suite), `&#32;`
next to a blank is one node here and two chunks there, and libxml2's lookahead
is unreliable at 4 KiB chunk ends ("the NOBLANKS option essentially
unusable", parser.c), which a streaming reader cannot copy.

Getters borrow the reader's current node; nothing is allocated per node once
buffers warm up. Memory is the `BufferedReader`'s plus one node of lookahead
(text must see its successor to know it is complete). `expand`, `read_string`
and inner/outer XML buffer the current subtree; buffered node text above
`Limits::max_owned_bytes` fails with `LimitExceeded(OwnedBytes)`. That failure
is not sticky: the buffered nodes are replayed by later reads.

XML errors are sticky. An I/O error is returned once; later reads return
`InvalidState`. A text node complete before a failure is delivered first.

Parser properties report the options: `LoadDtd` is true when a resolver was
supplied and the flags load the external subset, `DefaultAttributes` is
`DTDATTR`, `SubstituteEntities` is `NOENT` and `Validate` is `DTDVALID`.
`set_parser_property` sets `DTDLOAD`, `DTDATTR`, `DTDVALID` or `NOENT` before
the first read and rebuilds the stream with the resolver kept (`InvalidState`
for a change afterwards; setting the current value always succeeds). Clearing
`LoadDtd` clears `DTDLOAD` only, so it reads true while `DTDATTR` or
`DTDVALID` loads the subset, as libxml2's `loadsubset` does. `LoadDtd` on
without a resolver is `UnsupportedOption`.

## DTD validation

`DTDVALID` in `with_options`, or `set_parser_property(Validate, true)` before
the first read, validates the document against its DTD as it is read, as
`xmlReaderForMemory(..., XML_PARSE_DTDVALID)` and `xmllint --stream --valid`
do. The source then loads what libxml2 loads to validate (the external
subset and external entities, through the resolver). A
`valid::StreamValidator` sees every source event,
including the content of kept entity references, and the reader passes it
the source's DTD after each event: the external subset is loaded while the
document element's start tag is read, so that tag is checked once its event
has been consumed. Diagnostics accumulate in `dtd_diagnostics` and can
concern the node of lookahead. `is_valid` is true while DTD or schema
validation is set and no validator has reported an error, as
`xmlTextReaderIsValid`.

`valid.rs` is one event engine (`src/valid/engine.rs`: `start`, `text`,
`other`, `flush`, `end`) that tree validation drives by walking the tree and
`StreamValidator` drives with parser events, as libxml2's push validation
(`xmlValidatePushElement`, `xmlValidatePushCData`, `xmlValidatePopElement`
over `vstateTab`) does. Each element type's `children` model is compiled once
per DTD into its Glushkov automaton (`src/valid/model.rs`, deterministic by
Appendix E, one transition lookup per child); an open element keeps its
declaration and its automaton state, and the attribute checks, `ID` table
and `IDREF` list are fed at the start tag. `IDREF`s are resolved when the
document element ends, as `xmlValidateDocumentFinal` does at the end of the
document. Memory is one small frame per open element (depth is bounded by
`Limits::max_depth`), the `ID` values and the `IDREF` attributes (bounded by
the input and entity limits), and the diagnostics.

Diagnostics come in document order, each as soon as it is known; `node` is
the element's index in document order. Tree validation reports the same
diagnostics, sorted into `xmlValidateDocument`'s preorder (an element's
content, its required attributes, its attribute values, then its
descendants) by a key the engine gives each. Differences in wording follow
libxml2's two modes:

- A content-model mismatch is worded as the push validator words it and
  reported at the child or end tag that shows it, once per element:
  `Element r content does not follow the DTD, Misplaced b`, `..., Text not
  allowed` or `..., Expecting more children`. The tree reports `Element r
  content does not follow the DTD` (libxml2's tree validation adds
  `expecting (a , b), got (b a)`, which needs the children). libxml2's reader
  reports most content problems twice, once from its parser
  (`xmlValidateOneElement` at the end tag) and once more per later child
  from the push validator; the reader here reports each once.
- No DTD, or a DTD that declares nothing, is `Validation failed: no DTD found
  !` at the document element and nothing else is checked, as libxml2's parser
  does; tree validation (`xmlValidateDocument`) says `no DTD found!` without
  a DTD and validates against an empty one.
- `IDREF attribute a references an unknown ID "x"`: libxml2's reader has no
  attribute node left to name and says `attribute a line N references ...`;
  without line numbers (they wait for `work/errors`) the tree's wording is kept.
- A blank `CDATA` section in element content is invalid in both modes (as
  libxml2's tree validation decides); libxml2's push validator only rejects
  non-blank character data, so its reader reports it once, not twice.

Differential results, with the pinned libxml2 source and the XMLTS suites
(`cargo test --release --test valid_stream -- --ignored`, `YAXML_XMLLINT`,
`YAXML_LIBXML2_SRC`, `YAXML_SUITES`): over libxml2's `test/valid`,
`test/errors` and `test/*.xml` and the 1,054 XMLTS `valid` and `invalid`
cases, read with and without `NOENT` (2,202 parsed runs), stream and tree
diagnostics are equal, content-model wording aside, except for the 4 runs
whose DTD declares nothing. Against `xmllint --stream --valid` 1,095 of 1,095
verdicts agree and for 1,087 documents every message the reader prints is
one libxml2 prints; against `xmllint --valid` 1,095 of 1,095 tree verdicts
agree and 1,083 message lists are equal (content-model wording aside). The
remaining messages are worded differently in both modes: parameter-entity
nesting (`Element declaration doesn't start and stop in the same entity`,
four files), `duplicate references of`/`to` in mixed content, the two blank
`CDATA` cases above, and an undeclared prefix in entity content
(`errors/ns-ent.xml`). `yaxmllint --stream --valid` agrees with `xmllint` on
stdout and exit code on all 120 documents of the `tests/yaxmllint.rs`
differential.

## Schema validation

`relaxng_set_schema(Some(&schema))`, before the first read, validates the
document as it is read with a [`relaxng::Stream`](relaxng.md#streaming-validation)
fed every event the reader takes from its source, including the content of
kept entity references and the DTD for `ENTITY` and `NOTATION` values. No
subtree is buffered for it (libxml2 expands one whenever
`xmlRelaxNGValidatePushElement` returns 0), and `expand` or `read_*` do not
validate twice. Diagnostics accumulate in `relaxng_diagnostics` and can
concern the node of lookahead; `is_valid` is true while there are none, and
false without a schema, as `xmlTextReaderIsValid` returns 0 then. Verdicts
equal tree validation's and `xmllint --stream --relaxng`'s on the libxml2
suite.

`xsd_set_schema(Some(&schema))` does the same with an [`xsd::StreamValidator`](xsd.md)
(it clones the schema's `Arc`, so the reader has no lifetime) fed by the same
points, and the DTD for `ID` and `ENTITY` values. Both validators can be set;
`is_valid` is true when at least one is set and none reported an error.
Diagnostics are in `xsd_diagnostics` and equal `Schema::validate_stream`'s on
the libxml2 `test/schemas` suite. `PushParser::dtd` and `BufferedReader::dtd`
supply the DTD to callers driving `relaxng::Stream::set_dtd` or
`xsd::StreamValidator::set_dtd` themselves.

## Differential results

```sh
ORACLE=/path/to/yaxml-oracle   # pinned libxml2 2.15.4 build and source
cc -std=c11 -Wall -Wextra -Werror -I$ORACLE/build \
  -I$ORACLE/libxml2-2.15.4/include tooling/libxml2_textreader.c \
  $ORACLE/build/libxml2.a -lm -lz -liconv -o target/textreader-helper
YAXML_TEXTREADER_HELPER="$PWD/target/textreader-helper" \
  cargo test --test textreader_oracle -- --nocapture
```

`cases_match_the_reader_helper` dumps every node of 24 documents (cursor
properties, `Standalone` and `IsDefault`, every attribute, the nodes of its
value through `ReadAttributeValue`, indexed and named lookups, base, language,
namespace lookups, `read_string`, inner/outer XML, and at the end `Standalone`,
`Normalization` and `ByteConsumed`) through both readers: 24 of 24 agree.
`option_cases_match_the_reader_helper` runs 42 documents under their own
flags (kept and substituted references in values, empty values and namespace
URIs, DTD defaults with and without `DTDATTR`, XML declarations, `NOBLANKS`
with `xml:space`, DTD content, references, comments and the text rule): 42 of
42 agree (the helper takes the flags as a second argument). On malformed input libxml2 parses ahead in chunks and can
fail before delivering nodes, so its delivered nodes need only prefix ours.
`rde_results_match` replays libxml2's own `result/*.rde` reader expectations
(`NOENT`; `YAXML_LIBXML2_SRC`, default the pinned source tree): 129 of 132
agree. The three differences are EBCDIC and EUC-JP input (unsupported
encodings) and `ent11`, where `&#13;` in replacement text stays CR (see
[compatibility](compatibility.md#document-type-declarations)).
`rdr_results_match` replays the default-option `result/*.rdr` files, with
their EntityReference nodes: 130 of 132 agree, the two encodings differing.
`noblanks_streams_match_xmllint` (`YAXML_XMLLINT`) runs
`xmllint --stream --debug --noblanks` over the same inputs: 131 of 132 agree
(`iso-8859-5.xml`: the pinned xmllint lacks the encoding), and 128 of 132 with
`--noent` (the encodings, `ent2` and `ent10`: replacement-text context above).

## Gaps

- Error handlers (`SetErrorHandler`, `SetStructuredErrorHandler`,
  `GetErrorHandler`, `GetLastError`) and the locator functions: errors are
  returned, and `Error::describe` gives libxml2's record
  ([bindings.md](bindings.md#error-mapping)).
- `line_number`/`column_number` (`GetParserLineNumber`/`ColumnNumber`) give
  the parsing point (input consumed, ahead of the current node by one node
  of lookahead) in a caller's `LineIndex`, since the reader keeps no input;
  libxml2's point also depends on its 512-byte chunking.
- `CurrentNode`, `CurrentDoc`, `Preserve` and `PreservePattern` do not fit.
  They hand out pointers into the tree libxml2's SAX handlers build as the
  reader advances, which the reader frees behind itself: `CurrentDoc` is
  whatever has not been freed yet (preserved nodes and their ancestors, plus
  the parse-ahead within the 4 KiB chunk), a definition by implementation
  accident that cannot be tested against the oracle byte for byte. This reader
  keeps no tree, only the queued events of one node and the subtree a `read_*`
  buffered; a borrowed node that outlives the next `read` is what `&mut self`
  forbids. `expand` is the safe form: an owned copy of the current subtree,
  which a caller keeps or drops. A pattern-driven collector (`pattern::StreamMatcher`
  plus `expand` at each match) would be a new API, not `PreservePattern`.
- `NOBLANKS` differences: above. `RECOVER` recovers (above). `XINCLUDE` is
  rejected.
- Attributes are not kept for EndElement nodes: libxml2 lets the `move_to_*`
  calls visit them there.
- A PI with only whitespace after its target (`<?p ?>`) has no value and
  serializes as `<?p?>`; libxml2 keeps an empty value and a space.
- Inner/outer XML return `None` on DocumentType and EndElement nodes, where
  libxml2 dumps the DTD or a partially freed element.
- `expand` returns a standalone copy; its source ranges are offsets into the
  outer XML, and nodes other than elements do not expand.
- Not started: `ConstEncoding`, `ConstXmlVersion`, `ConstString`,
  `IsNamespaceDecl`, `QuoteChar` (always `"` in libxml2), `NextSibling`.
  `Close` is `Drop`.
