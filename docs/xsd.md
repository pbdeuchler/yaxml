# XML Schema

`yaxml::xsd` ports libxml2 2.15.4 `xmlschemas.c`: XML Schema 1.0 documents
compile to an immutable, `Send + Sync`, reusable `Schema` (`xmlSchemaParse`)
that validates either DOM (`xmlSchemaValidateDoc`). Diagnostics carry
libxml2's numeric `XML_SCHEMAP_*`/`XML_SCHEMAV_*` codes (`xsd::Code`) and
message wording.

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
configurations without backtracking. `validate.rs` walks the instance as
`xmlSchemaVDocWalk` does (xsi:type, xsi:nil, defaults and fixed values,
wildcards, ID/IDREF), and `idc.rs` evaluates `unique`/`key`/`keyref` with
streaming `pattern` matchers and libxml2's node tables.

## Limits

`xsd::Limits`: per-document XML limits, `max_documents` (256) and
`max_bytes` (64 MiB) across all resolver loads, `max_depth` (256) for
particle nesting through groups, `max_model_work` (2^24 units for all content
models; exceeding it is `XML_SCHEMAP_INTERNAL`) and `max_counter_vectors`
(1024 per element; exceeding it is `XML_SCHEMAV_INTERNAL`). Circularity checks
are linear; include/import cycles load each document once.

## Agreement with libxml2

- libxml2 `test/schemas` (`tests/xsd_suite.rs`, runtest protocol): verdicts
  agree on all 233 instance tests, with equal error counts on all 233.
- `tests/xsd.rs`: 221 table cases with exact code sequences, all
  checked against libxml2 through `tooling/libxml2_xsd.c`.
- `tests/xsd_random.rs`: random schemas and instances against the same
  helper; 0 differences in 20,000 cases on the default seed.

## Differences

- libxml2's epsilon reduction lets a counted repetition inside a nullable
  loop, such as `(x{2,3})*`, end below `minOccurs`; yaxml reports the missing
  children (about 1 random case in 2,500).
- libxml2's counter execution rejects some valid instances of nested counted
  groups (`seq{3}(choice*(seq{0,2}(any?), c{1,2}))` rejects `<c/>`); yaxml
  accepts them (about 1 random case in 30,000).
- Content models over `max_model_work` fail instead of compiling slowly
  (libxml2 needs 30 s for 2,500 optional elements in a sequence; yaxml
  compiles that in about 1 s and rejects 5,000), and group nesting stops at
  `max_depth`.
- No streaming (`xmlSchemaValidateStream`) or SAX plug; `xml:base` in schema
  documents is ignored when resolving locations; attribute-use value
  constraints are not checked against the declaration's, as in libxml2.
