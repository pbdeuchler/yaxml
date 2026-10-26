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
// Pass `Box::new(resolver)` to `ParseOptions::with_resolver`.
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
  max_depth: 50, max_local: 64 }`. A file over budget counts as missing.
- `add(type, orig, replace)`, `remove(value)` and `dump()` are
  `xmlACatalogAdd`, `xmlACatalogRemove` and `xmlACatalogDump`; see
  [Editing and dumping](#editing-and-dumping).
- With `CATALOG_PI`, `CatalogResolver` adds the catalogs a document names; see
  [Document catalogs](#document-catalogs-catalog_pi).

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

## Editing and dumping

`Catalog::add(type, orig, replace)` is `xmlACatalogAdd`. An XML catalog takes
the element names of its entries (`public`, `system`, `rewriteSystem`,
`delegatePublic`, `delegateSystem`, `uri`, `rewriteURI`, `delegateURI`,
`nextCatalog`) and `catalog` (accepted, consulted by no lookup); `orig` is the
identifier or prefix, `None` for `nextCatalog`. An entry of the same type
whose stored name equals `orig` changes in place (a public identifier compares
unnormalized with the normalized stored one, so `"a  b"` never updates `"a b"`,
and a delegate that already loaded keeps its file); any other is appended to
the root file's entries. `replace` is used as given, not resolved against the
catalog's URL, and a delegate added this way prefers `public`. An unknown type
is `Error::Rejected`. An SGML catalog takes its keywords (`PUBLIC`, `SYSTEM`,
`DELEGATE`, `ENTITY`, `DOCTYPE`, `LINKTYPE`, `NOTATION`, `SGMLDECL`,
`DOCUMENT`, `CATALOG`, `BASE`) and needs a name no entry holds; only
`PUBLIC` and `SYSTEM` entries answer lookups, but the others take their key
and are dumped and removed like them.

`Catalog::remove(value)` is `xmlACatalogRemove`: it retires every entry of an
XML catalog's root file whose name or target (as written) is `value`, or
removes the SGML entry with that key, and returns how many. libxml2 returns 0
for XML. A removed group takes its members out of the dump but not out of
lookups.

`Catalog::dump()` is `xmlACatalogDump` returning the text: an XML catalog as
the indented catalog document with the OASIS document type declaration (entries
as written, `group` with `id`, `xml:base` and `prefer`; removed entries and
delegates or `nextCatalog`s whose file failed to load are left out), an SGML
catalog as one line per entry. Attribute values escape as libxml2's do for a
document without an encoding (non-ASCII as `&#xE9;`).

`Catalog::diagnostics()` lists what libxml2's `xmlCatalogErr` raises while
XML catalog files are read, in load order, each with its file's URL: an entry
lacking its identifier or URI (`CATALOG_MISSING_ATTR`, once per missing
attribute; the entry is skipped), a URI that does not resolve against its base
(`CATALOG_ENTRY_BROKEN`; skipped), a `prefer` other than `public` or `system`
(`CATALOG_PREFER_VALUE`; the inherited preference stays) and a loaded
`nextCatalog` or delegate file whose root is not an OASIS `catalog`
(`CATALOG_NOT_CATALOG`; line 0 where libxml2 has -1). Domain, code, level,
line, `str1`, `str2` and message match libxml2's structured errors; at most 100
are kept. The root file fails with `Error::NotCatalog` instead. libxml2's
`CATALOG_RECURSION` (a per-entry depth counter, reported with the entry's
usually NULL name) is not reported: loops and nesting past `max_depth` are cut
silently.

Differences, each measured against the oracle:

- libxml2 parses an XML catalog on the first lookup, add or removal, so dumping
  before then prints an empty catalog; here the root file is parsed when the
  catalog is built, and a dump always prints it. The differential looks up
  something first.
- libxml2 stops dumping a group when its first member is a delegate or
  `nextCatalog` that failed to load (its dump loop meets a "catalog" entry in
  the starting position and follows its empty child list), so the group prints
  empty. That drops entries from a dump, which defeats dump-and-reload, so
  members are all printed. The oracle generator keeps delegates and
  `nextCatalog` out of groups for this reason.
- The SGML dump is in key order; libxml2's is its hash order.
- `remove` returns the count (above). An XML catalog cannot be created empty;
  parse `<catalog xmlns="urn:oasis:names:tc:entity:xmlns:xml:catalog"/>`
  (libxml2's `xmlNewCatalog(0)` makes a catalog that refuses every `add`).
- **Declined: `xmlCatalogConvert` / `xmlConvertSGMLCatalog`.** In 2.15.4 the
  scan passes `&catal` instead of `catal` to its callback, which reads the
  address of a local as a catalog: on the oracle build, converting a loaded SGML
  catalog with `PUBLIC`, `SYSTEM`, `ENTITY` and `DELEGATE` entries segfaults
  (exit 139). The callback also returns at once when the catalog has no XML
  part, which an SGML catalog never has, so the intended conversion moves
  nothing either. There is nothing to match and no behavior to invent.
- The functions on the process-wide catalog (`xmlCatalogAdd`, `xmlCatalogRemove`,
  `xmlCatalogDump`, `xmlCatalogDumpDoc`, `xmlLoadCatalogs`, ...) have no
  counterpart because there is no global catalog.

## Document catalogs (`CATALOG_PI`)

With `options::CATALOG_PI`, each `<?oasis-xml-catalog catalog="URL"?>`
outside the DTD (prolog, content, or an entity's replacement text, as in
libxml2's `xmlParsePI`) goes to `Resolver::catalog_pi` on every parsing surface
(`Reader`, `PushParser`, `BufferedReader`, `TextReader`, both DOMs). The
default ignores it, so it does something only if the caller's resolver is a
`CatalogResolver`, which then adds the catalog as `xmlCatalogAddLocal` does:
document catalogs are consulted in document order before the main catalog
(`xmlResolveFromCatalog`: document identifiers, main identifiers, document URI
rules, main URI rules), and apply to loads that happen after the instruction.
The resolver is consumed by the parse, so the catalogs last exactly that long.

Capability safety: the URL is passed unchanged (libxml2 does not resolve it
against the document either) to the catalog's own resolver as
`ResourceKind::Catalog`, so a root-confined `FileResolver` refuses `../x.xml`,
absolute paths and URLs, and anything else the caller's resolver forbids fails
the same way: the catalog is ignored from then on and lookups go on. Loads
count against `Limits::max_loads` and `max_bytes` like every other catalog
file, and the list stops growing at `Limits::max_local`. `NO_XXE` still stops
every load before the resolver is asked. The syntax is `xmlParseCatalogPI`'s
(`catalog = "URL"` or single quotes, blanks around, nothing after); anything
else adds nothing.

Tests: `tests/catalog_pi.rs` (every surface, ordering, bounds, a root-confined
resolver, a resolver that ignores the hook) and a differential against
`xmlReadFile` with `XML_PARSE_CATALOG_PI` (`harness/tooling/libxml2_catalog.c`, op
`parse`): 800 generated parses, 93 of them changed by a document catalog,
0 mismatches (also 0 over 15 other seeds).

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
0 mismatches (also 0 over four other seeds, 19,200 more lookups, and 0
over eight more).

The edit differential (`edits_and_dumps_match_libxml2`, same helper, ops
`add`, `del`, `dump`) applies 24 random operations to each of 400 catalogs
and compares return codes, lookups and dumps (SGML dumps as sorted lines):
9,542 operations with 1,270 dumps, 0 mismatches (also 0 over 28 more seeds).
`YAXML_CATALOG_SEED` changes the seed of either test. Writing it found two
older differences in lookups, now fixed: a lookup with no usable identifier
(an empty public identifier alone) loaded `nextCatalog` files, which libxml2
does not, and an SGML `PUBLIC ""` added by `add` answered a query for `""`.

## Fuzzing

`tests/support/fuzz/catalog.rs` (target `catalog`) runs the lookups over fuzz-provided
catalogs and files, and, from `+type|orig|replace` and `-value` lines of its
last piece, edits and dumps them: every edit sequence must dump to a catalog
that loads and dumps the same after one reload, and edits cleaned to absolute
names on an empty XML and an empty SGML catalog must dump, load and answer
exactly as the edited catalog does. A 5 minute run on `nightly-2026-10-18`
(`-fork=1 -timeout=2`, 348,000 executions, 3 seeds added to the corpus) found
no crash, timeout or leak; its first run found two harness assumptions that do
not hold (a repeated lookup on one catalog may answer differently, because
lazy loads and failures change it, and a missing name dumps as `""`, which
loads as a name).

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
- `Catalog::is_empty` (`xmlCatalogIsEmpty`) is false for a loaded XML catalog
  with entries; libxml2 returns 1 until a lookup loads the file.
- `Catalog::load_super` (`xmlLoadSGMLSuperCatalog`) reads an SGML catalog
  whose `CATALOG` entries are kept unloaded under their written system
  literal (never resolved against `BASE`, never matched); XML content fails.
- Not ported: `xmlCatalogConvert` (declined, above) and debugging output.
