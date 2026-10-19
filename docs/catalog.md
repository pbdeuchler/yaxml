# Catalogs

`yaxml::catalog` ports libxml2 2.15.4 `catalog.c`: OASIS XML Catalogs and
SGML (TR9401) catalogs with libxml2's resolution order, not the
specification's where they differ. There is no global or default catalog and
nothing reads `/etc/xml/catalog` or `XML_CATALOG_FILES`.

```rust
use yaxml::catalog::{Catalog, CatalogResolver, Limits};
use yaxml::FileResolver;

let files = FileResolver::new("catalogs")?;
let catalog = Catalog::load("catalog.xml", Limits::default(), Box::new(files.clone()))?;
let resolver = CatalogResolver::new(catalog, Box::new(files));
// Pass `Box::new(resolver)` to any `with_resolver` constructor.
```

- `Catalog::parse(bytes, limits, url, resolver)` / `Catalog::load(url, limits,
  resolver)` are `xmlLoadACatalog`: content whose first `<`, `-` or ASCII
  letter is `<` is XML, anything else SGML. `url` bases relative entries.
- `resolve(public, system)` is `xmlACatalogResolve` (and, with one
  identifier, `ResolvePublic` / `ResolveSystem`); `resolve_uri` is
  `xmlACatalogResolveURI`. Both take `&mut self`: referenced catalogs load on
  first use and stay cached by URL; one that fails to load is ignored from
  then on, so a delegate that failed cuts only the first lookup, as in C.
- `CatalogResolver` maps each `Request` like libxml2's default entity loader
  (`xmlCatalogResolve(public, uri)`, then `xmlCatalogResolveURI(uri)`) and lets
  the inner resolver fetch the result, or the original URI.
- Catalog files are requested with `ResourceKind::Catalog` through the
  catalog's own resolver, within `Limits { max_loads: 64, max_bytes: 16 MiB,
  max_depth: 50 }`. A file over budget counts as missing.

## Behavior kept from libxml2

- Entries: `public`, `system`, `rewriteSystem`, `delegatePublic`,
  `delegateSystem`, `uri`, `rewriteURI`, `delegateURI`, `nextCatalog`,
  `group` (flattened; `prefer`, `xml:base`). `systemSuffix` and `uriSuffix`
  (XML Catalogs 1.1) are ignored because libxml2 ignores them.
- Order: system match, longest `rewriteSystem`, `delegateSystem` (cut), public
  match, `delegatePublic` with `prefer="public"` (cut), then `nextCatalog`s in
  order. `prefer` only affects `delegatePublic`; `public` entries match even
  under `prefer="system"`. URI lookups also follow matching `delegateSystem`
  entries, but only a matching `delegateURI` starts that step.
- Public identifiers are normalized (XML Catalogs 6.2) in entries and queries;
  `urn:publicid:` identifiers are unwrapped (uppercase escapes only, 2000-byte
  buffer); an identifier that normalizes to nothing matches no `public` entry
  but a `delegatePublic` with an empty prefix matches everything.
- Attributes are found by local name in any namespace (`xmlGetProp`);
  repeated `nextCatalog` entries with the same resolved URL, preference and
  group are dropped; up to 50 distinct delegates are remembered per step
  (`MAX_DELEGATE`).
- Cycles end through libxml2's per-query visited set of (catalog URL, public,
  system); `max_depth` additionally bounds nesting (libxml2 bounds only a
  single catalog's re-entry at 50).
- SGML: `PUBLIC` and `SYSTEM` entries share one key space (first wins),
  `CATALOG` files load eagerly and once, `BASE` replaces the base,
  `DELEGATE`, `ENTITY`, `DOCTYPE`, `LINKTYPE`, `NOTATION`, `DOCUMENT`,
  `SGMLDECL` and `OVERRIDE` are parsed and dropped. `ENTITY %name`, an
  unterminated comment, a non-public-id character in a literal or an
  unquoted literal at end of input rejects the catalog, as libxml2 does.
  `resolve_uri` consults `SYSTEM` entries.

## Tests

`tests/catalog.rs` runs libxml2's `test/catalogs` scripts through an
`xmlcatalog --shell` emulation (6 of 6 match `result/catalogs`) and table
tests for each entry type, traversal rule, limit and error. The differential:

```sh
ORACLE=/path/to/yaxml-oracle   # pinned libxml2 2.15.4 build and source
cc -std=c11 -Wall -Wextra -Werror -I$ORACLE/build \
  -I$ORACLE/libxml2-2.15.4/include tooling/libxml2_catalog.c \
  $ORACLE/build/libxml2.a -lm -lz -liconv -o target/catalog-helper
YAXML_CATALOG_HELPER="$PWD/target/catalog-helper" \
  cargo test --test catalog_oracle -- --nocapture
```

It writes 400 generated catalog sets (XML with groups, bases, delegates,
cycles, broken and truncated files; SGML with nesting) and compares 4,800
lookups in order, so lazy loading and broken-entry state are compared too:
0 mismatches (also 0 over four other seeds, 19,200 more lookups).

## Differences and gaps

- A malformed root XML catalog is `Error::Malformed`; libxml2 loads XML
  catalogs lazily and resolves nothing.
- Catalog XML is parsed by yaxml: encodings, namespace well-formedness and
  DTD attribute defaults (applied by element QName, not local name) follow
  yaxml's parser rather than libxml2's.
- `CatalogResolver` consults the catalog even when the original URI exists
  (libxml2 skips catalogs for existing local files). Entities inside a mapped
  resource are based on the original URI, not the mapped one, because
  `Resolver` cannot report the URI it served.
- Not ported: catalog editing and serialization (`xmlACatalogAdd/Remove/Dump`,
  `xmlcatalog --add/--del/--create`), SGML super catalogs, `xmlCatalogConvert`
  (its 2.15.4 implementation passes the wrong pointer), the `oasis-xml-catalog`
  processing instruction (`XML_PARSE_CATALOG_PI`) and debugging output.
