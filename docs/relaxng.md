# RELAX NG

`yaxml::relaxng` validates documents against RELAX NG schemas in the XML
syntax, the job of libxml2 2.15.4's `relaxng.c`, with a different engine:
James Clark's derivative algorithm over hash-consed patterns instead of
libxml2's backtracking.

| Call | libxml2 equivalent |
| --- | --- |
| `Schema::parse(bytes, base, resolver, &limits)` | `xmlRelaxNGNewMemParserCtxt` + `xmlRelaxNGSetResourceLoader` + `xmlRelaxNGParse` |
| `Schema::from_tree(&document, base, resolver, &limits)` | `xmlRelaxNGNewDocParserCtxt` + `xmlRelaxNGParse` (either DOM) |
| `schema.validate(&document)` | `xmlRelaxNGValidateDoc` (either DOM) |
| `schema.stream()`, then `stream.event(&event, origin)` per event | `xmlRelaxNGValidatePushElement`, `PushCData`, `PopElement`; `FullElement` is never needed |
| `TextReader::relaxng_set_schema`, `relaxng_diagnostics`, `is_valid` | `xmlTextReaderRelaxNGSetSchema` (`RelaxNGValidate` is `Schema::parse` plus this), `xmlTextReaderIsValid` |

A `Schema` is immutable, `Send + Sync` and reusable. Schema errors are
`SchemaErrors`, a non-empty list of `SchemaError { code, uri, offset,
message }` whose `SchemaCode` discriminant is libxml2's `XML_RNGP_*` number.
Only the first error is reported. Its code and message are libxml2's first
error (`xmlRelaxNGSetParserStructuredErrors`), except in these cases:
- libxml2 does not report its follow-on errors.
- An included or external schema that is not XML is `IncludeFailure` or
  `ExternalRefFailure`. libxml2 reports the XML parse error first.
- `data/except//ref` to a define whose content is not an element is
  accepted, as the specification allows. libxml2 rejects it.
- A RELAX NG element inside `name` is `TextExpected`. libxml2 accepts it and
  reads the name from the text.
- A `param` with content in a library without parameters is `TextExpected`.
  libxml2 reports `ParamForbidden` first.
Validation returns `Vec<Diagnostic<Id>>`, empty when valid; each `Code`
discriminant is libxml2's `xmlRelaxNGValidErr` (`UnknownId` is valid.c's
`XML_DTD_UNKNOWN_ID`), with the element and its source offset.

```rust
use yaxml::relaxng::{Code, Limits, Schema};

let rng = br#"<element name="r" xmlns="http://relaxng.org/ns/structure/1.0">
  <interleave><element name="a"><text/></element><element name="b"><empty/></element></interleave>
</element>"#;
let none = |_: yaxml::Request<'_>| Ok(None);
let schema = Schema::parse(rng, "r.rng", Box::new(none), &Limits::default())?;
assert!(schema.validate(&yaxml::Document::parse("<r><b/><a>x</a></r>")?).is_empty());
let [error] = schema.validate(&yaxml::Document::parse("<r><b/></r>")?).try_into().unwrap();
assert_eq!(error.code, Code::MissingElement);
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Schema processing

`load.rs` applies RELAX NG sections 4.1 to 4.11 while reading the schema DOM:
foreign elements and attributes are dropped (qualified attributes in the
RELAX NG namespace and unknown unqualified ones are errors), `name`, `type`
and `combine` are trimmed, `datatypeLibrary` (an absolute URI, or empty) and
`ns` are inherited, `xml:base` and `href` resolve with `xmlBuildURI`, and
`externalRef`/`include` documents are requested as `ResourceKind::RelaxNg`
from the resolver. An included grammar loses the `start` and `define`s the
`include` overrides (each must exist) and becomes a `div`. Recursion is an
error; nesting is bounded by `Limits::max_include_depth` (64) and documents
by `Limits::xml.max_external_loads` and `max_external_bytes`.

`compile.rs` does sections 4.12 to 4.21: children grouped, `mixed`,
`optional` and `zeroOrMore` rewritten, name class constraints (`anyName` and
`nsName` excepts, `xmlns` attribute names), `combine` with
`choice`/`interleave` checks, nested grammars with `parentRef`, and every
define inlined except through elements; a reference cycle without an element
is an error unless only unreachable definitions take part (4.19).
`notAllowed` and `empty` simplify in the pattern constructors. Patterns nest
at most 256 deep, counting inlined definitions.

`check.rs` then checks section 7 on the reachable schema: the prohibited
paths of 7.1 (`attribute//attribute`, `oneOrMore//group//attribute`,
`list//list`, `data/except//…`, `start//…` and the rest), content types
(7.2), overlapping attributes and `anyName`/`nsName` attributes outside
`oneOrMore` (7.3), and overlapping elements or text in `interleave` (7.4).

Datatypes: the built-in library (`""`, also under the RELAX NG namespace as
in libxml2) has `string` and `token` without parameters; the XML Schema
library has the 45 built-in types of `schema_types`, with every facet except
`enumeration` and `whiteSpace` as a parameter. Values compare in their
datatype's value space (`QName` values in the schema's namespace context).
As libxml2 does, `xsd:ID` attribute values must be unique and `xsd:IDREF(S)`
attribute values must name one; `ENTITY`, `ENTITIES` and `NOTATION` consult
the instance's DTD.

## Validation and bounds

Validation walks the tree with an explicit stack, computing derivatives for
start tags, attributes, text and end tags as in Clark's paper. Choices are
canonical (sorted, duplicate-free, balanced), patterns are interned, and the
start-tag, end-tag and `after` derivatives are memoized per validation, so
interleave-heavy schemas explore only the states a document reaches: 200
interleaved optional elements in reverse order validate in about 10 ms. Every
new pattern, memo miss and merged choice alternative spends one unit of
`Limits::max_work` (100 million by default, separately for building and for
each validation); when it runs out, validation stops with one
`Code::Internal` diagnostic and schema building fails with
`SchemaCode::ResourceLimit`.

After an error, validation continues: an unexpected element is skipped, a bad
attribute is ignored, missing attributes and content are forgiven, and an
element reports at most one content error. So yaxml reports one diagnostic per
fault where libxml2 often adds a chain of "failed to validate content" errors
for the ancestors.

## Streaming validation

`Schema::stream` returns a `Stream` that validates one document from its
events (`Start`, `Empty`, `End`, `Text`, `Cdata`; the rest are ignored), so
the slice `Reader`, `PushParser`, `BufferedReader` and `TextReader` validate
without a tree. The tree walk feeds the same engine the same calls (start tag
with attributes, text, end tag), so for the same document and options the
diagnostics are identical, except that `node` is the element's index in
document order. Each fault is reported when its event arrives; `xsd:IDREF`
references, and the `Internal` diagnostic of an exhausted budget, when the
document element ends. `ENTITY`, `ENTITIES` and `NOTATION` values need the
DTD: `Stream::set_dtd` takes `BufferedReader::dtd()` after the `Doctype`
event (`TextReader` does this itself).

libxml2 compiles each element's content to an automaton for push validation;
where it cannot (`interleave`, name classes other than a single name, and the
other cases `xmlRelaxNGIsCompilable` rejects), `PushElement` returns 0 and the
caller must expand the subtree and call `xmlRelaxNGValidateFullElement`,
which `xmlTextReader` does by buffering it. Derivatives consume one event at a
time for every pattern, so yaxml buffers no subtree. Memory is the open
elements' frames and namespace bindings (bounded by `Limits::max_depth`) and
each one's text since its last child tag, plus the derivative memo and interned
names, which cost work units and so stay within `Limits::max_work`, and the
`xsd:ID` values and pending references, which libxml2 keeps too. After an
element the content does not allow, its subtree is skipped by depth count.

## Results

`YAXML_LIBXML2_SOURCE=../yaxml-oracle/libxml2-2.15.4 YAXML_XMLLINT=../yaxml-oracle/build/xmllint cargo test --release --test relaxng -- --include-ignored --nocapture`:

| Suite | Cases | Expected verdict | Same as libxml2 |
| --- | ---: | ---: | ---: |
| libxml2 `test/relaxng` instances (`result/relaxng`) | 159 | 159 | 159 |
| James Clark `spectest.xml` schemas | 373 | 368 | 371 |
| `spectest.xml` instances | 529 | 528 | 529 |
| libxml2 `testsuite.xml` schemas / instances | 28 / 183 | 28 / 183 | 28 / 183 |
| `xsdtest/xsdtestsuite.xml` schemas / instances | 238 / 1,035 | 238 / 1,033 | 238 / 1,034 |

On the 42 invalid libxml2 instances libxml2 reports 65 validity errors and
yaxml 48, mostly in the same categories (wrong element, namespace, attribute,
datatype, list, value); libxml2's extra ones are the ancestor chains above.
The first message is libxml2's on 26 of the 42. Like libxml2, a datatype
failure reports its innermost cause first: `TypeValue` (lexical), `Data`
(facet or `except`), `TypeCompare` (a `value` whose instance does not parse,
or whose type has no computed value, such as `string`), `Value`, `ListExtra`
(the token after the last complete prefix of a list), `Datatype` (a missing
list item), and `DataElement`/`ValueElement`/`ListElement` for a child element
in such content; attribute values report the same inner cause. The other 16
differ in libxml2's choice and interleave backtracking.
The differences from the expected verdicts:

- Five `spectest.xml` schemas use names such as U+0E35 that XML 1.0 Fifth
  Edition allows and the 2001 suite does not; libxml2 accepts them too.
- Stricter than libxml2, as the specification requires: `datatypeLibrary="foo:"`
  is not an absolute URI, and an attribute whose name class is a choice
  containing `nsName` must be inside `oneOrMore`. An unprefixed `QName` value
  takes the `ns` attribute as default namespace (libxml2 rejects
  `xsdtestsuite` case 98145).
- `NOTATION` data without a DTD is invalid, as in libxml2 (two `xsdtest`
  instances expect it valid).

Streaming over `BufferedReader` events gives exactly the DOM's diagnostics
(code, offset, message) on every instance above: 154 libxml2 instances with a
compiling schema and 529, 183 and 1,035 suite instances (`tests/relaxng.rs`
also streams every table case through the slice `Reader` and through
`PushParser` one byte at a time). `libxml2_relaxng_suite_streams_like_xmllint`
compares `TextReader` validation with `xmllint --stream --relaxng`: 154 of 154
verdicts agree, and 122 of 154 message lists, all 112 valid instances and 10
of the 42 invalid ones. The others differ as in tree mode, plus libxml2's
push-mode wording (`Did not expect text in element  TODO  content`, `Extra
data in list`). `yaxmllint --stream --relaxng` matches `xmllint`'s exit code
and stdout on all 798 schema and instance pairs of `tests/yaxmllint.rs`.

## Gaps

- No compact syntax or user-registered datatype libraries.
- `xmlRelaxNGDump` and `xmlRelaxNGDumpTree` are declined; see below.
- The DTD compatibility annotations (`a:defaultValue`) are ignored and IDs
  are checked as libxml2 does, not with the ID-type consistency rules of
  the DTD compatibility specification.
- Messages follow libxml2's wording; inside `choice` and `interleave` the
  message chosen can differ.

## Declined: `xmlRelaxNGDump`, `xmlRelaxNGDumpTree`

Neither prints a simplified grammar, so there is nothing yaxml's compiled
schema could print in its place (checked on the 2.15.4 oracle build, which has
`LIBXML_DEBUG_ENABLED`).

- `xmlRelaxNGDump` is a debug printer of libxml2's internal `define` tree: it
  writes `RelaxNG: <url>`, an open `<grammar>`, and the `<start>` pattern. It
  prints nothing for `data`, `value`, `except` and `param`, never prints the
  `define`s (its own `TODO`), and prints an empty `<start>` when the start
  is a `ref`: for a grammar with `<start><ref name="r"/></start>` and an
  element-defining `r` the whole body is `<start>\n</start>`.
- `xmlRelaxNGDumpTree` is `xmlDocDump` of the schema *document* after
  `xmlRelaxNGCleanupTree` rewrote it in place: an element's or attribute's
  `name` attribute becomes a `<name ns="">` child, while `optional`, `mixed`,
  `zeroOrMore` and foreign attributes stay as written. The grammar above prints
  as one line, `<define name="r"><element><name ns="">r</name>...`. Matching it
  byte for byte means reproducing that rewrite (and what it does to
  `include`, `externalRef`, `div` and `ns` inheritance) and `xmlDocDump`'s
  output of the result.
- yaxml keeps neither: `load.rs` reads the document into a transient `El` tree
  while it applies sections 4.1 to 4.11, `compile.rs` turns that into
  hash-consed derivative patterns with defines inlined, and the `El` tree is
  dropped. A dump of either would be a new serializer for a tree kept alive
  only for it, matched against a debug format nothing consumes (both are
  `P3` debug aids; the grammar is already available as the schema text).

## Fuzzing

`tests/support/fuzz/relaxng.rs` (target `relaxng`) splits a testcase at NUL into schema,
instance and resources `d/0`..`d/9`, wraps non-markup schemas in an
element, and checks determinism, identical diagnostics over both DOMs and on
reuse, offsets matching node ranges, the load budget, at most one trailing
`Internal` diagnostic, that a four times larger work budget changes no
completed result, and that streaming validation over `BufferedReader` (with
the DTD) and over `PushParser` chunks reports exactly the DOM's diagnostics.
Seeds are the libxml2 and spec suite cases. Two 331 s runs on `nightly-2026-06-06` (`-timeout=2`, about 2.0 and 1.8 million executions, 16,429 edges) found no crash, timeout or leak. With the streaming checks, a
427 s run (`-fork=1 -timeout=2`, about 3.4 million executions, `cov` 26,330)
found none either.
