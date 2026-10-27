# XML Schema

`yaxml::xsd` ports libxml2 2.15.4 `xmlschemas.c`: XML Schema 1.0 documents
compile to an immutable, `Send + Sync`, reusable `Schema` (`xmlSchemaParse`)
that validates either DOM (`xmlSchemaValidateDoc`) or a stream of parser
events without building a tree (`xmlSchemaValidateStream`, `xmlSchemaSAXPlug`).
Diagnostics carry libxml2's numeric `XML_SCHEMAP_*`/`XML_SCHEMAV_*` codes
(`xsd::Code`) and message wording.

```rust
use yaxml::xsd::{self, Code, Schema};

let schema = Schema::parse(br#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <xs:element name="r"><xs:complexType><xs:sequence>
    <xs:element name="n" type="xs:int" maxOccurs="3"/>
  </xs:sequence></xs:complexType></xs:element></xs:schema>"#, "r.xsd")?;
let document = yaxml::Document::parse("<r><n>1</n><n>x</n></r>")?;
let diagnostics = schema.validate(&document);
assert!(!xsd::is_valid(&diagnostics));
assert_eq!(diagnostics[0].code, Code::CvcDatatypeValid1_2_1);
# Ok::<(), Box<dyn std::error::Error>>(())
```

## API

- `Schema::parse(bytes, uri)`: one document; `include` fails and `import`
  is skipped. `Schema::with_resolver(bytes, uri, resolver, limits)` loads
  `include`, `import` and `redefine` targets through the caller's `Resolver`
  (`ResourceKind::Schema`). `Schema::from_instance(tree, uri, resolver,
  limits)` builds a schema from the instance's `xsi:schemaLocation` and
  `xsi:noNamespaceSchemaLocation` hints; hints are never followed otherwise.
- `Err(Error)` holds every `SchemaDiagnostic` (code, URI, offset, message);
  any error fails the whole schema, as in libxml2. `Schema::warnings()`
  keeps the rest.
- `Schema::validate(&tree)` returns `Vec<Diagnostic<Id>>` (code, element id,
  source offset, message); `xsd::is_valid` ignores warnings.
  `Schema::validate_element(&tree, id)` validates one subtree as the
  validation root (`xmlSchemaValidateOneElement`; `None` for a non-element).
  `Schema::validate_creating(&mut editor)` validates an `OwnedDocument` and
  then adds the schema's defaults to it (`XML_SCHEMA_VAL_VC_I_CREATE`): absent
  attributes with a default or fixed value, normalized by their type, and the
  default value of each empty, non-nilled element as a text child.
- `Schema::validate_stream(input, options)` parses a `BufRead` with
  `BufferedReader` and validates in the same pass (`xmlSchemaValidateStream`).
  `StreamValidator::new(&schema)` (no lifetime: `Schema` clones share one `Arc` of compiled data) takes each `Event` and its `SourceOrigin`
  from any streaming surface (`Reader`, `PushParser`, `BufferedReader`), as
  libxml2's SAX plug does, and `finish` returns the diagnostics. They equal
  `validate`'s for the parsed tree, with `node` the element's index in
  document order. Default and fixed values are checked and feed identity
  constraints but are never inserted, in both modes.

## Components

`Schema::element(ns, name)`, `attribute`, `type_definition` (also finds the
built-in types) and the sorted lists `elements()`, `attributes()`,
`type_definitions()` (global types only) are read-only views of the compiled
components, `schemasInternals.h`'s `xmlSchemaElement`, `xmlSchemaAttribute`
and `xmlSchemaType`. Imported components are found by the same lookups.
`TypeDefinition` gives the name, base, derivation method, `final`/`block`
sets, abstractness, and either the simple variety (atomic, list item, union
members) or the complex content type with its effective attribute uses
(inherited ones included, prohibited ones dropped) and attribute wildcard.
A simple type also gives its whitespace handling (`None` for a union, as
libxml2 sets no flag), its effective facets (`facetSet`: inherited
length, bound, digit and `whiteSpace` facets included, as typed
`schema_types::Facet` values, not lexical) and this derivation step's
patterns; enumerations have no view.
`ElementDeclaration` gives the type, nillable, abstract, `block` and value
constraint; `AttributeUse::value` is the use's own constraint, not the
declaration's, as in libxml2. tests/xsd_components.rs prints them in the line
format of harness/tooling/libxml2_xsd_components.c, which walks libxml2's
hash tables; with `YAXML_XSD_COMPONENTS_HELPER` the ignored test compares all
123 libxml2 `test/schemas` schemas that both build. Differences: a union
derived by restriction lists its base's member types (libxml2 leaves
`memberTypes` empty), and wildcard namespace sets are sorted, absent
namespace first (libxml2 keeps document order). Not exposed: substitution
groups, particles and model groups, attribute groups (flattened into uses),
enumerations, identity constraints, notations, annotations and the schema's own
`targetNamespace`/`version`.

## Pipeline

`parse.rs` reads documents breadth first (schema-for-schemas checks, target
namespace and chameleon rules, form/block/final defaults) into drafts with
unresolved QNames. `fixup.rs` follows libxml2's fixup order: redefinitions,
reference resolution, circularity, simple types and facets, attribute value
constraints, complex types (content types, attribute uses and wildcards,
`derivation-ok-restriction` 2 to 5), element declarations and substitution
groups, then content models. `automaton.rs` rebuilds libxml2's content-model
automaton (`xmlSchemaBuildAContentModel`, epsilon elimination,
`xmlFAComputesDeterminism`) only to decide Unique Particle Attribution as
libxml2 does, including which of two equal particles it keeps. `model.rs`
compiles the model actually matched: a Glushkov automaton with counters, so
`maxOccurs="1000000"` is one counter, matched over a capped set of
configurations without backtracking. `validate.rs` validates start, text and
end events as `xmlSchemaVDocWalk` and the SAX plug do (trees feed it their
nodes, streams their events: one engine for both), handling xsi:type,
xsi:nil, defaults and fixed values, wildcards and ID/IDREF; `idc.rs` evaluates
`unique`/`key`/`keyref` with streaming `pattern` matchers and libxml2's node tables.

## Limits

`xsd::Limits`: per-document XML limits, `max_documents` (256) and
`max_bytes` (64 MiB) across all resolver loads, `max_depth` (256) for
particle nesting through groups, `max_model_work` (2^24 units for all content
models; exceeding it is `XML_SCHEMAP_INTERNAL`), `max_model_bytes` (64 MiB
for the entries the compiled models and the automaton compiling the current
one hold; exceeding it is `XML_SCHEMAP_INTERNAL` too) and `max_counter_vectors`
(1024 per element; exceeding it is `XML_SCHEMAV_INTERNAL`) and `max_idc_keys`
(2^20 identity-constraint key sequences per validation; beyond it validation
reports `XML_SCHEMAV_INTERNAL` once and stops keeping keys and checking
keyrefs). Circularity checks are linear; include/import cycles load each document once.

## Agreement with libxml2

- libxml2 `test/schemas` (`harness/tests/xsd_suite.rs`, runtest protocol): verdicts
  agree on all 233 instance tests, with equal error counts on all 233.
- `tests/xsd.rs`: 223 table cases with exact code sequences, all
  checked against libxml2 through `harness/tooling/libxml2_xsd.c`.
- `tests/xsd_random.rs`: random schemas and instances against the same
  helper; 0 differences in 20,000 cases on the default seed.
- Stream validation equals tree validation (codes, offsets, messages) on all
  of the above, plus `tests/xsd_stream.rs` over the buffered, push (byte by
  byte) and slice readers. libxml2's own stream validation
  (`xmlSchemaValidateStream` through the helper's `stream` mode, and
  `xmllint --stream --schema`) agrees on all 204 `test/schemas` instances whose
  schema builds, the 20,000 random cases (with the counted-loop quirk below) and
  every table case but two (`ID` and `ENTITY`, below).
- `TextReader::xsd_set_schema` feeds the same events as the reader takes from
  its source: its diagnostics equal `validate_stream`'s on all 233 suite
  instances (`harness/tests/xsd_suite.rs`), and `yaxmllint --stream --schema` ends as
  `xmllint --stream --schema` does (exit code, verdict line, error count) on all
  204, with identical stderr on 183; the rest differ in message wording (facet
  messages, enumeration sets, keyref element prefix) and reader line numbers.

## Differences

- **Created namespaced defaults.** With `XML_SCHEMA_VAL_VC_I_CREATE`,
  libxml2 gives a qualified default attribute the innermost declaration of
  its URI even when that is the default namespace, so the saved attribute is
  unqualified. `validate_creating` takes the innermost *prefixed* binding,
  else declares `p0`, `p1`, ... on the document element as libxml2 does when
  none is in scope.
- **Counter quirks, deliberately not reproduced.** libxml2's epsilon
  reduction lets a counted repetition inside a nullable loop, such as
  `(x{2,3})*`, end below `minOccurs`, so it accepts an invalid instance; yaxml
  reports the missing children (about 1 random case in 2,500). libxml2's
  counter execution rejects some valid instances of nested counted groups
  (`seq{3}(choice*(seq{0,2}(any?), c{1,2}))` rejects `<c/>`); yaxml accepts
  them (about 1 random case in 30,000). Both are validity bugs, which the
  project does not match, and neither is cheap to match: they are artifacts
  of `xmlRegExecPushString` running counters over the epsilon-reduced
  automaton with backtracking. `automaton.rs` builds that automaton only to
  decide UPA, so matching them would mean porting the executor as a second
  engine beside `model.rs` and keeping its state in both the tree and the
  stream path, for outcomes no schema author wants. The two are reported as
  the only divergence classes of the random differential, and are bounded
  (about 1 case in 1,000 of that generator overall).
- Content models over `max_model_work` or `max_model_bytes` fail instead of
  compiling slowly or holding hundreds of MB, and group nesting stops at
  `max_depth`. Both libxml2 and yaxml hold transitions quadratic in the
  particles for a sequence of optional elements: libxml2 needs 17 s and
  126 MB for 2,000 of them and 170 s and 468 MB for 4,000, both accepted;
  yaxml compiles up to 1,741 (0.35 s, 132 MB resident) and rejects longer
  ones within about 115 MB.
- `xml:base` in schema documents is ignored when resolving locations;
  attribute-use value constraints are not checked against the declaration's,
  as in libxml2.
- **Stream `ID` and `ENTITY`, deliberately not matched.** libxml2's stream
  validation has no nodes, so it never checks `ID` uniqueness (it accepts a
  document that repeats an `ID`, which violates cvc-id) and rejects every
  `ENTITY` value, declared or not (it rejects a valid document). Matching
  would make yaxml's stream mode wrong in both directions about validity, and
  the stream already has what the tree has (the `ID` table, the DTD's
  unparsed entities), so yaxml's keeps the tree's verdicts. The two table cases
  that differ (`duplicate ID values`, `ENTITY names an unparsed entity`) are
  pinned by `libxml2_stream_agrees_except_ids_and_entities`, which fails if
  either verdict ever changes. Streams see the DTD of the internal subset only (`ID` attributes
  and unparsed entities of an external subset are unknown), and since a tree
  knows every ID beforehand, a schema `ID` repeated by a later DTD ID or
  `xml:id` is reported at the later attribute in a stream. `StreamValidator::set_dtd`
  replaces the internal subset with a reader's fuller DTD, as `TextReader`
  does.
- Derivation by restriction must restate a base attribute's `fixed` value
  (derivation-ok-restriction 2.1.3). libxml2 2.15.4 compares the base's value
  with itself and never reports this, so it accepts schemas that omit or change
  the fixed value; yaxml and Xerces reject them.
