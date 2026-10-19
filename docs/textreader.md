# TextReader: libxml2's `xmlreader.h` cursor

`TextReader<R: BufRead>` is the `xmlTextReader` node cursor that lxml
`iterparse`, `Nokogiri::XML::Reader` and similar bindings drive. It wraps a
[`BufferedReader`](streaming.md), so it reads UTF-8, UTF-16 and the built-in
single-byte encodings from `&[u8]` or any `BufRead` in bounded memory.

| libxml2 | `TextReader` |
| --- | --- |
| `xmlReaderForMemory`, `xmlReaderForIO` | `new`, `with_limits`, `with_decode_limits`, `with_base_uri` |
| `xmlTextReaderSetResourceLoader` | `with_resolver(base, resolver)` |
| `Read`, `Next`, `ReadState` | `read`, `next`, `read_state` |
| `NodeType`, `Depth`, `IsEmptyElement`, `HasValue`, `HasAttributes`, `AttributeCount` | same names, snake case |
| `ConstName`, `ConstLocalName`, `ConstPrefix`, `ConstNamespaceUri`, `ConstValue`, `ConstXmlLang` | `name`, `local_name`, `prefix`, `namespace_uri`, `value`, `xml_lang` (borrowed `&str`) |
| `BaseUri`, `LookupNamespace` | `base_uri` (owned), `lookup_namespace` |
| `GetAttribute`, `GetAttributeNo`, `GetAttributeNs` | `get_attribute`, `get_attribute_no`, `get_attribute_ns` |
| `MoveToAttribute`, `…No`, `…Ns`, `MoveToFirstAttribute`, `MoveToNextAttribute`, `MoveToElement` | `move_to_*` returning `bool` |
| `ReadString`, `ReadInnerXml`, `ReadOuterXml`, `Expand` | `read_string`, `read_inner_xml`, `read_outer_xml`, `expand` (an `OwnedDocument`) |
| `GetParserProp`, `SetParserProp` | `parser_property`, `set_parser_property` |

`NodeType`, `ReadState` and `ParserProperty` carry libxml2's integer values
(`as i32`). The reader produces Element, Attribute, Text, CDATA, PI, Comment,
DocumentType, SignificantWhitespace and EndElement nodes.

## Semantics

The reader behaves like `xmlReaderForMemory(..., XML_PARSE_NOENT |
XML_PARSE_DTDATTR)`: entities are substituted (no EntityReference nodes) and
DTD default attributes appear. Following libxml2:

- `<a/>` is one Element with `is_empty_element`; `<a></a>` has an EndElement.
- Adjacent text, including entity replacement text, is one node. Text made only
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
- `read_string` is the text of a Text/CDATA node, or the concatenated
  descendant text of an element with children (`None` for an empty element).
- Inner/outer XML follow `xmlNodeDump` of a copy: childless elements are
  `<a/>`; text escapes `<`, `>`, `&` and CR; attributes also escape `"`, tab and
  LF; namespaces used inside the copy but declared outside it are added to the
  copy's root after its own declarations, in first-use order. Inner XML
  serializes each child as its own copy.
- `expand` parses the outer XML into an `OwnedDocument` under the reader's
  limits. The cursor does not move: later reads still visit the subtree.

Getters borrow the reader's current node; nothing is allocated per node once
buffers warm up. Memory is the `BufferedReader`'s plus one node of lookahead
(text must see its successor to know it is complete). `expand`, `read_string`
and inner/outer XML buffer the current subtree; buffered node text above
`Limits::max_owned_bytes` fails with `LimitExceeded(OwnedBytes)`. That failure
is not sticky: the buffered nodes are replayed by later reads.

XML errors are sticky. An I/O error is returned once; later reads return
`InvalidState`. A text node complete before a failure is delivered first.

Parser properties report the fixed behavior. `LoadDtd` is true exactly when a
resolver was supplied, `DefaultAttributes` and `SubstituteEntities` are always
true and `Validate` false. Setting any property to another value returns
`ErrorKind::UnsupportedOption`.

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
properties, every attribute, indexed and named lookups, base, language,
namespace lookups, `read_string` and inner/outer XML) through both readers:
24 of 24 agree. On malformed input libxml2 parses ahead in chunks and can
fail before delivering nodes, so its delivered nodes need only prefix ours.
`rde_results_match` replays libxml2's own `result/*.rde` reader expectations
(`YAXML_LIBXML2_SRC`, default the pinned source tree): 129 of 132 agree. The
three differences are EBCDIC and EUC-JP input (unsupported encodings) and
`ent11`, where `&#13;` in replacement text stays CR (see
[compatibility](compatibility.md#document-type-declarations)).

## Gaps

- No EntityReference nodes (`XML_PARSE_NOENT` off), no `XML_PARSE_NOBLANKS`,
  no DTD, RELAX NG or XML Schema validation, no `xmlTextReaderPreserve`,
  patterns, `ReadAttributeValue`, `Close`, line/column or encoding getters.
- Attributes are not kept for EndElement nodes: libxml2 lets the `move_to_*`
  calls visit them there.
- A PI with only whitespace after its target (`<?p ?>`) has no value and
  serializes as `<?p?>`; libxml2 keeps an empty value and a space.
- Inner/outer XML return `None` on DocumentType and EndElement nodes, where
  libxml2 dumps the DTD or a partially freed element.
- `expand` returns a standalone copy; its source ranges are offsets into the
  outer XML, and nodes other than elements do not expand.
