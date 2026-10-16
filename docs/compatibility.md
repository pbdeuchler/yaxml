# Compatibility catalogue and implementation order

This is a work inventory, not a claim of implemented compatibility. The baseline is **libxml2 2.15.4**, released 2026-09-04. Pin a source release, build options, and option combinations for every differential test; libxml2 behavior and available symbols vary by release and configuration. The [official release directory](https://download.gnome.org/sources/libxml2/2.15/) identifies this release. The [tagged headers](https://github.com/GNOME/libxml2/tree/v2.15.4/include/libxml) define the source API.

The adjacent [`libxml2-api.tsv`](libxml2-api.tsv) indexes **4,051 identifiers** across the 46 tagged public header/template files. Four compatibility headers contain no API declarations and have explicit `empty_header` rows. This document groups the required behavior and orders implementation. Nothing below is implicitly out of scope because it is low priority.

## Meaning of replacement

1. **Behavioral replacement:** equivalent XML results, events, mutations, queries, serialization, validation, and failure classification through a safe Rust API.
2. **Source migration:** familiar operations and option mappings with explicit ownership and errors. C function names are a migration index, not the recommended Rust interface.
3. **Binary replacement:** exported C symbols, ABI layouts, ownership, callbacks, global variables, allocator hooks, variadic functions, and direct mutable struct fields. This cannot be implemented faithfully by a crate that forbids unsafe: arbitrary foreign pointers cannot be dereferenced safely. Treat this as a separate future adapter with its own policy decision, not an achieved property of the safe core.

Python, Ruby, JavaScript/TypeScript, and Go bindings should target owned document handles and document-scoped node identifiers. Foreign references must keep their document alive; deletion must reject stale handles. Avoid exposing the internal tree layout. WebAssembly with copied inputs/results offers one binding route without requiring unsafe Rust in this crate. Native runtime adapters ordinarily need dependencies and unsafe foreign interfaces outside the core.

The implemented subset now includes the borrowing slice reader, read-only DOM,
callback `PushParser`, bounded lending `BufferedReader`, owned trees, UTF-8 event
writing, complete borrowed/owned DOM serialization, bounded owned-value editing,
and an explicit `Read`
adapter. Input shares XML 1.0 grammar and rejects DTDs; owning/streaming input
also supports [UTF-16](encoding.md). See the [streaming contract](streaming.md)
for event lifetimes, limits, cancellation and incremental error ordering. The
full upstream reader/SAX/save option surfaces remain open.

The [owned edit API](owned-dom.md#editing-existing-values) replaces literal leaf
and existing ordinary-attribute values by checked handle. It preserves names,
namespace declarations, attribute order and topology. It does not implement
entity-interpreting setters, QName-based attribute creation,
structural moves, namespace repair, or C entry points. The header inventory
remains an unmapped upstream catalogue, not a claim that these Rust methods
provide full `tree.h` compatibility.

Recorded parsing differences include encoding agreement and namespace URIs. Detected
UTF-16 declarations must agree strictly, while libxml2's detected-encoding path
can warn and continue. Namespace URI syntax is not validated: an exploratory
2.15.3 run with `--strict-namespace` rejected `xmlns='urn:中'`, which yaxml
accepts. That observation applies to both UTF-8 and UTF-16; it is not a pinned
2.15.4 result. Encoding corpus fixtures use ASCII namespace URIs to isolate
decoding, and this URI-validation gap remains part of the compatibility burndown.

## Prioritized feature map

Priority is dependency order and migration value. P0 is the first usable secure XML subset; P1 adds true buffered streaming and ordinary editing/output; P2 enables XML document-processing stacks; P3 covers HTML, schemas and legacy clients; P4 adds language adapters.

| Priority | Feature and behavior to implement | Upstream API families / headers |
| --- | --- | --- |
| P0 | XML 1.0 strict well-formedness: declarations, names and Unicode name ranges, one root, nesting, comments, PI, CDATA, text, attributes, empty tags, legal characters, EOF and trailing content | `parser.h`, `parserInternals.h`, `chvalid.h`, `xmlstring.h`; `xmlRead*`, `xmlCtxtRead*`, `xmlCtxtParseDocument`, `xmlCtxtParseContent` |
| P0 | Entity/reference handling: five predefined entities, decimal/hex numeric references, attribute-value normalization, CR/CRLF normalization, character validity after decoding, rejection of undeclared entities | `entities.h`, parser SAX callbacks |
| P0 | Namespaces: scoped bindings, expanded names, prefix rebinding, default namespaces, reserved prefixes/URIs, duplicate expanded attribute names; preserve lexical prefixes and declaration order | `tree.h`, `SAX2.h`; `xmlNewNs`, `xmlSearchNs*`, `xmlReconciliateNs` |
| P0/P1 | P0 borrowed slice pull events; P1 buffered pull and push chunks, EOF distinct from need-more-input, stop/cancel, bounded buffering, every byte boundary including UTF-8 and delimiter splits | `xmlreader.h`, `parser.h`, `SAX2.h`; `xmlReaderFor*`, `xmlTextReaderRead`, `xmlCreatePushParserCtxt`, `xmlParseChunk` |
| P0 | Read-only DOM: document/element/attribute/text/comment/CDATA/PI identity, parent/sibling/child traversal, namespace and attribute lookup, text-content concatenation, source positions | `tree.h`; `xmlDoc`, `xmlNode`, `xmlAttr`, `xmlNs`, `xmlDocGetRootElement`, `xmlGetProp`, `xmlNodeGetContent` |
| P0 | Errors and security budgets: structured domains/codes/severity, byte and line/column positions, limits on input/token size, depth, nodes, attributes, allocation and work; no implicit external-resource access | `xmlerror.h`, `xmlCtxtSetErrorHandler`, `xmlCtxtSetResourceLoader`, `xmlCtxtSetMaxAmplification` |
| P1 | Encoding: BOM/declaration/explicit-override precedence; UTF-8/UTF-16 LE/BE, ASCII; incremental decoder and original-byte offsets; then remaining upstream built-ins and caller-provided converters | `encoding.h`; `xmlCharEncoding`, `xmlCreateCharEncodingHandler`, `xmlCtxtSetCharEncConvImpl` |
| P2 | Full DTD syntax and non-validating semantics: internal/external subsets, parameter/general entities, declarations, notations, unparsed entities, default/fixed attributes, ID typing, standalone constraints, entity-reference preservation vs substitution | `entities.h`, `valid.h`, `parser.h`, `SAX2.h`; `xmlParseDTD`, `xmlGetDocEntity`, `xmlGetID` |
| P1 | Mutable DOM: creation, insertion, unlink/remove, replacement, attributes, namespaces, copy/import/adopt, cross-document ownership, adjacent-text merging, base URI, `xml:lang`, `xml:space`, line metadata | `tree.h`; `xmlAddChild*`, `xmlAddSibling`, `xmlUnlinkNode`, `xmlReplaceNode`, `xmlSetProp`, `xmlDocCopyNode`, `xmlDOMWrap*` |
| P1 | Serialization and writer: escaping, declarations, encoding, empty-element spelling, CDATA boundaries, namespace fixup, formatting, incremental writes and I/O failures; preserve semantic round trips | `xmlsave.h`, `xmlwriter.h`, `tree.h`; `xmlSaveTo*`, `xmlSaveDoc`, `xmlTextWriter*`, `xmlDocDump*` |
| P1 | Rust I/O and contexts: slices, strings, `Read`/`BufRead`, `Write`, context reuse/reset, explicit resource resolver, deterministic cleanup, parser options and capability reporting | `xmlIO.h`, `parser.h`; `xmlReadMemory`, `xmlReadIO`, `xmlCtxtReset`, `xmlCtxtSetOptions`, `xmlHasFeature` |
| P2 | DTD validation: content models, attribute types, defaults/fixed values, required attributes, IDs/IDREFs, entities and notations; separate well-formedness from validity | `valid.h`, `xmlautomata.h`, `xmlregexp.h`; `xmlValidateDocument`, `xmlValidateDtd`, `xmlValidCtxt` |
| P2 | XPath 1.0: parser/compiler/evaluator, all axes, predicates, document order/deduplication, namespace nodes, four value types and conversions, core functions, variable/namespace/custom-function registration, limits | `xpath.h`, `xpathInternals.h`; `xmlXPathEvalExpression`, `xmlXPathCompile`, `xmlXPathCompiledEval`, `xmlXPathRegister*` |
| P3 | HTML parsing and serialization: libxml2's actual recovery/tree behavior, entities, implied elements, case folding, void/raw-text elements, encoding rules, push/SAX support | `HTMLparser.h`, `HTMLtree.h`; `htmlRead*`, `htmlCtxtRead*`, `htmlParseChunk`, `htmlDocDump*` |
| P2 | Canonicalization: Canonical XML 1.0/1.1 and exclusive C14N, node subsets, comments toggle, inclusive namespace prefixes, exact bytes | `c14n.h`; `xmlC14NDocDumpMemory`, `xmlC14NDocSaveTo`, `xmlC14NExecute` |
| P3 | XML Schema 1.0: parse/compile schemas, imports/includes/redefines, simple/complex types, facets, derivation, identity constraints, defaults, document/stream validation and diagnostics | `xmlschemas.h`, `schemasInternals.h`, `xmlschemastypes.h`; `xmlSchemaParse`, `xmlSchemaValidateDoc`, `xmlSchemaValidateStream`, `xmlSchemaVal*` |
| P3 | RELAX NG: XML syntax, include/externalRef, patterns, datatype libraries, document and push validation, diagnostics | `relaxng.h`; `xmlRelaxNGParse`, `xmlRelaxNGValidateDoc`, `xmlRelaxNGValidatePush*` |
| P2 | URI and resource resolution: parse/build/escape/normalize/resolve/relative URIs; base URI rules; XML and SGML catalogs, rewrite/delegate/nextCatalog, per-context resolver policy | `uri.h`, `catalog.h`; `xmlBuildURI`, `xmlParseURI`, `xmlCatalogResolve*`, `xmlACatalog*` |
| P2 | XInclude 1.0: XML/text inclusion, base/language fixup, fallback, recursive inclusion/cycle limits, resolver permissions, XPointer interaction | `xinclude.h`; `xmlXIncludeProcess*`, `xmlXIncludeNewContext` |
| P2/P3 | P2 patterns: streaming pattern matching, namespace-aware patterns; P3 XPointer: supported schemes and node selections | `pattern.h`, `xpointer.h`; `xmlPatterncompile`, `xmlPatternMatch`, `xmlStreamPush*`, `xmlXPtrEval` |
| P3 | XML recovery modes, deprecated SAX1 and parser-internal entry points: reproduce only against named option/release fixtures; malformed-input recovery is not a specification | `SAX.h`, `parserInternals.h`, deprecated `xmlParse*`, `xmlRecover*`, `xmlSAX*` |
| P3 | Schematron validation and reporting, XLink callbacks, public regex/automata/datatype helper APIs | `schematron.h`, `xlink.h`, `xmlregexp.h`, `xmlautomata.h`, `xmlschemastypes.h` |
| P3 | Public utility APIs: dictionaries/interning, hashes, lists, buffers, UTF-8/string helpers, Unicode categories/blocks, diagnostic tree dumps/shell | `dict.h`, `hash.h`, `list.h`, `tree.h`, `xmlstring.h`, `xmlunicode.h`, `debugXML.h` |
| P3 | Legacy process/thread globals, initialization/cleanup, memory debugging and callbacks, dynamic modules, removed HTTP/FTP compatibility stubs, build/version/export macros | `globals.h`, `threads.h`, `xmlmemory.h`, `xmlmodule.h`, `nanohttp.h`, `nanoftp.h`, `xmlversion.h`, `xmlexports.h` |
| P4 | Language adapters and migration packages: document/node ownership, byte/text conversion, structured exceptions, iteration, cancellation, cleanup and batching across runtime boundaries | Python, Ruby, JavaScript/TypeScript, Go; separate from safe-core dependencies |

The whole surface includes callbacks, enum values, macros, typedef aliases and public struct members, not just functions. Exact function signatures and member layouts remain available in the pinned headers; the TSV is an index rather than a C header replacement.

## Option compatibility

Maintain an explicit mapping for **every** `xmlParserOption`, HTML parser option, reader property, save/writer flag and schema/validation option. Each mapping needs default, supported modes, interactions, and tests. In particular, cover `NOENT`, `DTDLOAD`, `DTDATTR`, `DTDVALID`, `NO_XXE`, `NONET`, `NOBLANKS`, `RECOVER`, `HUGE`, `NODICT`, `NSCLEAN`, `NOCDATA`, `COMPACT`, `BIG_LINES`, `IGNORE_ENC`, `OLD10`, `NOBASEFIX`, `XINCLUDE`, `UNZIP`, and catalog flags. Never silently accept an unimplemented option.

Match observable success behavior first: event/tree shape, namespace identity, normalized values, text coalescing, entity visibility and serialization bytes where specified. Error code/domain/severity and position matter next. Exact human error wording, allocation count, callback text-chunk boundaries and historical recovery quirks need separate documented contracts; streaming may split text into different chunks while preserving concatenated content.

## Constraints and deliberate boundaries

The safe core can use `std`, safe collections and memchr. This restriction does not automatically permit unsafe code in local adapters or new dependencies. SIMD inside the allowed memchr dependency follows that dependency's implementation policy; the prohibition should be enforced on yaxml source with `forbid(unsafe_code)`.

libxml2 optionally uses iconv/ICU, zlib and platform dynamic-loading functions. Under the dependency policy, built-in legacy encodings and compression require original safe implementations, generated checked-in tables with provenance, or explicit caller-provided conversion/decompression outside the core. A resolver cannot grant automatic network/file access just because input contains a URI. Native dynamic loading cannot be reproduced safely with only standard-library safe APIs. Preserve an explicit unsupported result until a permitted solution exists.

The [2.15.0 release announcement](https://discourse.gnome.org/t/libxml2-2-15-0-released/31397) records removal of the built-in HTTP client and LZMA, Schematron disabled by default, and planned removals. Keep removed/legacy stubs in the inventory without making their old implementation a requirement for the default profile. XSLT is supplied by libxslt, not libxml2; an XSLT engine is outside this catalogue. XML Schema 1.1 and a complete WHATWG HTML5 tree builder are not implied by libxml2 compatibility.

No implementation can promise superior performance on every workload or prove the absence of vulnerabilities with fuzzing coverage alone. Publish measurements per workload and an explicit coverage denominator. Every public input surface, option family, decoder, state transition, mutation and serializer needs a fuzz target or a documented exclusion. Unreached code is a burndown item, not permission to claim 100%.

## Acceptance and burndown

For each feature, track implementation, happy-path tests, malformed/adversarial tests, differential cases, fuzz target and measured coverage, then benchmarks. A feature is complete only when its observable contract is recorded and those checks pass. Unsupported features return structured errors; permissive parsing must never discard unsupported declarations silently.

Start with the bounded UTF-8 XML scanner and pull events, then construct the read-only DOM from the same semantic parser. Add namespaces and normalization before advertising an ordinary XML parser. Make streaming chunk invariance, error determinism, limits, and panic freedom release gates from the first slice. Add encodings, mutation and output next; layer DTDs, XPath, validators and inclusion over the resulting shared data model. Keep the low-level scanner internal so malformed syntax cannot bypass semantic checks through another public facade.

## API inventory method and limits

The inventory contains 1,404 functions, 18 variables, 86 callback typedefs, 202 other typedefs, 48 enums, 1,288 enum values, 284 macros, 55 named structs, and 666 member paths. It includes deprecated compatibility declarations and declarations hidden behind build options. The `status` column is `unmapped`: listing a C identifier does not claim that a corresponding Rust operation or C symbol is implemented. Priority ranges identify modules spanning several roadmap stages; the feature table determines operation-level order.

`deprecated_marker=yes` records an adjacent `XML_DEPRECATED` attribute or a member's `XML_DEPRECATED_MEMBER`. `not_marked` does **not** mean supported, recommended or non-deprecated: upstream also describes deprecations in documentation comments. This column is a mechanical hint, not a full lifecycle classification. For example, the parser context's public members include documented deprecations without compiler attributes.

This is a lexical source sweep with an audit, not an ABI extractor. It strips C comments, collects `XMLPUBFUN`/`XMLPUBVAR` declarations, callback and simple typedefs, enums and enumerators, named structs and member declarations, and non-underscore `#define` identifiers. It removes preprocessor directives before reading member names and excludes the export macro's `__declspec` expansion. Nested encoding-handler unions use qualified paths such as `_xmlCharEncodingHandler.input.func`; function-pointer members are fields, while named callback typedefs are callbacks. Include guards beginning with underscores are excluded. Internal compatibility macros exposed by public headers remain listed.

The extraction does not preprocess conditional builds, evaluate macro/enum values, duplicate full signatures, encode struct layouts, or establish platform symbol availability. `xmlversion.h.in` is the version/configuration template; a configured `xmlversion.h` must be checked separately for each oracle build. Historical releases and removed APIs absent from these headers need separate compatibility inventories if targeted. Before claiming complete C source or binary compatibility, compare against a C AST and built-library exports for each supported configuration. The tagged headers are authoritative for signatures and these remaining checks.

The canonical identifier checksum is `dbfde559ca150a1505455a98024c16fdfe2955e066de90ff5a225ac636d13cba`. To reproduce it from the TSV, omit `empty_header` rows, render each row as `header<TAB>identifier<TAB>kind<TAB>marker`, where marker is `1` for `yes` and `0` otherwise, sort the complete strings by Unicode code point, join with LF, append one LF, and compute SHA-256 over UTF-8. The result matched both the source extraction in the browser and the independently transferred local rows. This verifies transfer integrity; it does not prove semantic completeness.

The source text was read from GitHub's `v2.15.4` file-content view. Twenty headers were also retrieved through the web tool; normalization of blank lines there makes those copies unsuitable as original source-byte checksums. The table below hashes the UTF-8 text of each original file-content view. To rerun the sweep, obtain all 46 files from the pinned tag, check these hashes, apply the extraction rules above, and compare canonical rows. A maintained AST-based generator is a follow-up tooling task.

```text
header sha256
parser.h b4c2821d32f272b34e01572d3246388fb1be5b9bfe35696bae866a0b56a01c2b
HTMLparser.h 80a4faa9d65d4f5c6fe0e036ecde300f98fc3fdda92bdea3634e78cfc470304b
HTMLtree.h 251ca85dc41bdb9d8a00695fd7dc8b27e873739b3091d2b7aed168252f1c07d6
SAX.h f2a12a855c8c28f3c2cb7f6c671847969077683f9152ba603a1a8714fac6b724
SAX2.h 056172ae32592c59496b4b2d5f4bd57f93ce2f613de4708a6d67ef232e5be0b8
c14n.h d4dd43f23d0d99189885c819b019ff4cf9e20013494382278af15498d12495da
catalog.h d45c54511493eabb52f4da3afc170c071ec828a81fa4844da85a069c72ac752d
chvalid.h 2af7f1677a53b8e7b58e0b1d2ab6448161a1a83e94954ec7e5e8e2eb985807c2
debugXML.h b270cafe5e70078844eaf8e24462a90d41beab2ecdb2b8fc769f7bb91e0fbb0c
dict.h c5db5cf34e45b52836f78f5367e1425c34c0c823463a6cc89fbf33e7134e0d9b
encoding.h a209d2db9d54b8a633529323b61c88e6f68c260ba71e33507857679997a8b239
entities.h bd1c6b5698d7db685bb58b6dcf925dcaa2d05b75511d0ccd1d93b742ae4e6d53
globals.h e9f26df5b564287ef2a4f43ffd4b9a44b91e3c50ed691b8a5c0ee3aa5c7011d4
hash.h dfd5a891ceebed2991977be45f5201b003c3c747874a5036c162e3c4e15348e6
list.h f78454a27dddc90554d9f8be0b6cf02f81b1991b4d3281c8bf405c65041b7d9f
nanoftp.h a8a06436f1eee25321b9577abad5b9d821f66b3031105d37ba9bf60a3744d15e
nanohttp.h 912bf7293a554119227f8885984dc8b92638dd0728be068ee9879900f968fbc0
parserInternals.h 4a65dd93449a2c9034d3cc863f470170d29526500798a96cd6e91d0839d79b10
pattern.h cc7f600ed465679661f252fa761e525ee51620f1550abe8b3b960cb8d8ac7c27
relaxng.h ca487d06f72956d413a6bc3592639282704ebada603b5245d7b81551f7cc6d62
schemasInternals.h 50a37b6a5c70dff7221d7720b065522617e0cfe27ebd039faea7c9f9522cabaf
schematron.h 08c4992667b80585c879d90e69396f557e2d42571d20ff1b48133ea96d7f622c
threads.h f3e1f354437176cc64419d876d73824057fff3e0f98fee1f96e6217c4f28140c
tree.h 19ad178432433576ff2896ae93d65b1f3bba6820866936d91c51183019982721
uri.h 7f24c6bed123101d20da9efdc515cdf02768bc56b85b6ad3f10c83d834014fe4
valid.h 3890145eebc9eca52e4397e5c900e3e6932e3923ae872f8bec6c572f1ed3aeeb
xinclude.h be8634ee5267f6d20e449016fc54983c508f5268fd206f4e00f0091fa7e61946
xlink.h 3e1a5d534f0233f1d6456f45fc2e58f69a42ed4a724c581de6132153fe3f0204
xmlIO.h b585b16d015b8b4522ae5877c3ea24552aa37de7aa3c2d03d2712bab489cdcff
xmlautomata.h 234bdfb083f89f35938e79165930488a1b9eab517dfc302915b10d63c4f80556
xmlerror.h 8018f2f22b7d6d5309b14c755d61411f485d4cb961ad67a6ba0da64c9a71897f
xmlexports.h 68d4ff841221cc4d4299e19861d5163353b8a31898cd492953ff3c3d4b7a05dc
xmlmemory.h 7b545ef68b0b72ec94514e30baee8471783c328038e0ba279c6659587e09a6bd
xmlmodule.h d82f9204edbff8ff480609506fff2b7f888f0da98afacf96b2a7d860899d16b4
xmlreader.h dd34c0024a04da2af487918bd975489b3b816219758d128fda7f605ebd737f63
xmlregexp.h cf90aa97a1a87bc47c6575e01cd471db71f56812f2748d76c58e0c41df2535b3
xmlsave.h 09918bf7814dfc0ae0139a587c192551e7f5873f885935028f116a498c114bcc
xmlschemas.h 4977f45a01c13f30fd8a1edbb02bd9766aee6f493f832ee3a116b65b24a4a82a
xmlschemastypes.h 874e1377756ba07d1a8eac17e799cf59a3b5a9e0c27841db5bf7e2f4033b402f
xmlstring.h 90cb949305050fd0420d35594f1279a3349c3983b3e2a5aafc2794e1abf68c88
xmlunicode.h 9dc186b7f058fdd31bed2c3c923ad4980568e82f3b7d2903de3bc83e1e3d18ce
xmlversion.h.in 54ece9fdea9c0d7ae1af87aca6f7baff9abc848d4e6f661962a93de40b87f39c
xmlwriter.h 367e95d332e6617a6310bf28fc1941696a9330907600ada62303fdd954819a1c
xpath.h 1561ad68d4b85385eaaacb9a2f86ff024bf5b069f2e17943708d7d230d351202
xpathInternals.h ff33ac48412396540cc48984e19468b2f0e8d9c743e012f0f9cdf2e140e80556
xpointer.h 1d10e2a9a253b6d9188f56c3fa7e1830aa9b6e242075da8c6799f09206c4a433
```
