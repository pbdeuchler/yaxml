# Compatibility catalogue and implementation order

This is a work inventory, not a claim of implemented compatibility. The baseline is **libxml2 2.15.4**, released 2026-09-04. Pin a source release, build options, and option combinations for every differential test; libxml2 behavior and available symbols vary by release and configuration. The [official release directory](https://download.gnome.org/sources/libxml2/2.15/) identifies this release. The [tagged headers](https://github.com/GNOME/libxml2/tree/v2.15.4/include/libxml) define the source API.

The adjacent [`libxml2-api.tsv`](libxml2-api.tsv) indexes **4,056 identifiers** across the 46 tagged public header/template files. Four compatibility headers contain no API declarations and have explicit `empty_header` rows. This document groups the required behavior and orders implementation. Nothing below is implicitly out of scope because it is low priority.

## Meaning of replacement

1. **Behavioral replacement:** equivalent XML results, events, mutations, queries, serialization, validation, and failure classification through a safe Rust API.
2. **Source migration:** familiar operations and option mappings with explicit ownership and errors. C function names are a migration index, not the recommended Rust interface.
3. **Binary replacement:** exported C symbols, ABI layouts, ownership, callbacks, global variables, allocator hooks, variadic functions, and direct mutable struct fields. This cannot be implemented faithfully by a crate that forbids unsafe: arbitrary foreign pointers cannot be dereferenced safely. Treat this as a separate future adapter with its own policy decision, not an achieved property of the safe core.

Python, Ruby, JavaScript/TypeScript, and Go bindings should target owned document handles and document-scoped node identifiers. Foreign references must keep their document alive; deletion must reject stale handles. Avoid exposing the internal tree layout. WebAssembly with copied inputs/results offers one binding route without requiring unsafe Rust in this crate. Native runtime adapters ordinarily need dependencies and unsafe foreign interfaces outside the core.

The implemented subset now includes the borrowing slice reader, read-only DOM,
callback `PushParser`, bounded lending `BufferedReader`, owned trees, UTF-8 event
writing, complete borrowed/owned DOM serialization, bounded owned-value editing,
rooted document creation, fresh element/leaf insertion, permanent subtree removal, an explicit `Read`
adapter, [XPath 1.0](xpath.md) evaluation and [canonicalization](c14n.md) over both DOMs. Input shares XML 1.0 grammar, including the
internal DTD subset (below); owning/streaming input
also supports [UTF-16 and declared US-ASCII, ISO-8859-x and windows-1252](encoding.md). See the [streaming contract](streaming.md)
for event lifetimes, limits, cancellation and incremental error ordering. The
full upstream reader/SAX/save option surfaces remain open.

`Node::text_chunks()` and `OwnedNode::text_chunks()` provide allocation-free
string-value pieces for supported node kinds. Containers concatenate descendant
Text/CDATA logically; comment and PI leaves expose their own data. Joining the
pieces matches `xmlNodeGetContent` on the bounded [pinned oracle cases](validation-text-chunks.md).
The methods retain chunk boundaries and do not add entity nodes, attribute
nodes or the allocating C entry point.

## Document type declarations

Every parser reports one `Event::Doctype` (DOM: `NodeKind::Doctype` /
`OwnedNodeKind::DocumentType`, `doctype()`) with the name, unquoted public and
system identifiers and the exact internal-subset text. The internal subset is
processed: parameter entities, general entity substitution in content and
attribute values (re-tokenized, so replacement markup becomes events), default
and `#FIXED` attributes, and tokenized attribute-type normalization. External
subsets and external entities are recorded but never read; no resolver exists.

Defaults follow `xmlReadMemory(..., XML_PARSE_NOENT | XML_PARSE_DTDATTR)`.
Without `DTDATTR`, libxml2 does not insert default attributes; yaxml always
does, as XML 1.0 §5.1 requires of non-validating processors. Undeclared-entity
handling mirrors libxml2's `xmlHandleUndeclaredEntity`: once any PE reference
or an external subset is seen and the document is not `standalone='yes'`, an
undeclared general or parameter entity is dropped (libxml2 reports a
recoverable error), and references to unloaded external entities expand to
nothing (libxml2's failed load). Otherwise `UnknownEntity` is fatal. Like
libxml2, declarations after an unread PE reference are still processed.
Unparsed entities in content (`UnparsedEntity`), external entities in attribute
values (`ExternalEntityInAttribute`), recursion (`EntityLoop`), unbalanced
replacement markup (`UnbalancedEntity`) and malformed declarations
(`InvalidDtd`) are fatal. Ceilings are `Limits::max_dtd_bytes`,
`max_entity_expansion_bytes`, `max_entity_references` and `max_entity_depth`.

Known differences from the pinned oracle, all intentional: colons in entity or
notation names are a fatal `Namespace` error (libxml2 warns); PE references
inside internal-subset markup are fatal as the WFC requires; a `&#13;` in
replacement text stays CR in content (libxml2 parses entity text as new input
and turns it into LF); the serializer writes the DOCTYPE exactly as parsed
rather than regenerating declarations; no DTD validation, `xmlGetID` or
external loading. The differential cases live in `tests/dtd.rs`
(`YAXML_XMLLINT=... cargo test --test dtd -- --ignored`).

The [owned edit API](owned-dom.md#editing-existing-values) replaces literal leaf
and existing ordinary-attribute values by checked handle. Value edits preserve
topology. [Structural edits](owned-dom.md#leaf-insertion-and-permanent-removal)
append fresh Text/CDATA/comment/PI leaves and permanently remove non-root
subtrees. Text/CDATA require an element parent; comments/PIs also append to the
document epilog. PI targets and lexical payloads are validated before allocation.
Insertion never merges Text nodes; removal makes every subtree handle stale and
protects the document element and synthetic root. Parsed survivors retain source
provenance and created nodes have none. Creation and pool-work counters persist
across editors without refunds. [Rooted construction](owned-dom.md#creating-documents-and-elements)
also creates a complete document or appends a fresh empty element with ordered
initial attributes and declarations. Exact prefixes are preserved; automatic
prefixes follow a bounded deterministic policy. Each element retains one
contiguous attribute range. Existing names, namespace declarations and attribute
membership stay fixed. Existing-element attribute insertion/removal, sibling insertion,
moves/copy/import/adoption, detached ownership, slot reuse, compaction,
entity-interpreting setters, namespace repair and C entry points remain open.
In particular, these operations do not reproduce `xmlAddChild`'s move/adopt and
Text-merging behavior or `xmlUnlinkNode`'s detached ownership. The header inventory
remains an unmapped upstream catalogue, not a claim that these Rust methods
provide full `tree.h` compatibility.

Recorded parsing differences include encoding agreement and namespace URIs. Detected
UTF-16 declarations must agree strictly, while libxml2's detected-encoding path
can warn and continue. Namespace URI syntax is not validated: an exploratory
2.15.3 run with `--strict-namespace` rejected `xmlns='urn:中'`, which yaxml
accepts. That observation applies to both UTF-8 and UTF-16; it is not a pinned
2.15.4 result. Encoding corpus fixtures use ASCII namespace URIs to isolate
decoding, and this URI-validation gap remains part of the compatibility burndown.
Single-byte decoding follows libxml2 built without iconv/ICU but with
`LIBXML_ISO8859X_ENABLED`; the pinned oracle build disables ISO8859X, so it
reports ISO-8859-2..16 as unsupported while yaxml decodes them. A UTF-8 BOM
with a single-byte label is rejected where libxml2 warns and continues as UTF-8.

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
| P2 | XPath 1.0: parser/compiler/evaluator, all axes, predicates, document order/deduplication, namespace nodes, four value types and conversions, core functions, variable/namespace/custom-function registration, limits. **Implemented** over both DOMs; see [xpath.md](xpath.md) for the differential results, recorded differences and gaps (XPointer, `CHECKNS`/`NOVAR` flags, lookup callbacks) | `xpath.h`, `xpathInternals.h`; `xmlXPathEvalExpression`, `xmlXPathCompile`, `xmlXPathCompiledEval`, `xmlXPathRegister*` |
| P3 | HTML parsing and serialization: libxml2's actual recovery/tree behavior, entities, implied elements, case folding, void/raw-text elements, encoding rules, push/SAX support | `HTMLparser.h`, `HTMLtree.h`; `htmlRead*`, `htmlCtxtRead*`, `htmlParseChunk`, `htmlDocDump*` |
| P2 | Canonicalization: Canonical XML 1.0/1.1 and exclusive C14N, node subsets, comments toggle, inclusive namespace prefixes, exact bytes. **Implemented** over both DOMs; 50 of 73 libxml2 `test/c14n` cases match byte for byte, the other 23 need DTDs. See [c14n.md](c14n.md) for kept libxml2 quirks and gaps (visibility callback, file output, `xmlns=""` namespace nodes) | `c14n.h`; `xmlC14NDocDumpMemory`, `xmlC14NDocSaveTo`, `xmlC14NExecute` |
| P3 | XML Schema 1.0: parse/compile schemas, imports/includes/redefines, simple/complex types, facets, derivation, identity constraints, defaults, document/stream validation and diagnostics | `xmlschemas.h`, `schemasInternals.h`, `xmlschemastypes.h`; `xmlSchemaParse`, `xmlSchemaValidateDoc`, `xmlSchemaValidateStream`, `xmlSchemaVal*` |
| P3 | RELAX NG: XML syntax, include/externalRef, patterns, datatype libraries, document and push validation, diagnostics | `relaxng.h`; `xmlRelaxNGParse`, `xmlRelaxNGValidateDoc`, `xmlRelaxNGValidatePush*` |
| P2 | URI and resource resolution: `yaxml::uri` implements parse/save/escape/unescape/normalize/`xmlBuildURI`/`xmlBuildRelativeURI`/`xmlCanonicPath` for Unix ([uri.md](uri.md), differential-tested); still open: base URI rules in the DOM, XML and SGML catalogs, rewrite/delegate/nextCatalog, per-context resolver policy | `uri.h`, `catalog.h`; `xmlBuildURI`, `xmlParseURI`, `xmlCatalogResolve*`, `xmlACatalog*` |
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

The inventory contains 1,405 functions, 22 variables, 86 callback typedefs, 202 other typedefs, 48 enums, 1,288 enum values, 284 macros, 55 named structs, and 666 member paths. It includes deprecated compatibility declarations and declarations hidden behind build options. The `status` column is `unmapped`: listing a C identifier does not claim that a corresponding Rust operation or C symbol is implemented. Priority ranges identify modules spanning several roadmap stages; the feature table determines operation-level order.

`deprecated_marker=yes` records an adjacent `XML_DEPRECATED` attribute or a member's `XML_DEPRECATED_MEMBER`. Nested member paths inherit the marker of an enclosing member, such as `_xmlCharEncodingHandler.input`. `not_marked` does **not** mean supported, recommended or non-deprecated: upstream also describes deprecations in documentation comments. This column is a mechanical hint, not a full lifecycle classification. For example, the parser context's public members include documented deprecations without compiler attributes.

The initial lexical sweep strips C comments, collects `XMLPUBFUN`/`XMLPUBVAR` declarations, callback and simple typedefs, enums and enumerators, named structs and member declarations, and non-underscore `#define` identifiers. It removes preprocessor directives before reading member names and excludes the export macro's `__declspec` expansion. Nested encoding-handler unions use qualified paths such as `_xmlCharEncodingHandler.input.func`; function-pointer members are fields, while named callback typedefs are callbacks. Include guards beginning with underscores are excluded. Internal compatibility macros exposed by public headers remain listed.

A [configured Clang AST and static-symbol audit](validation/libxml2-api-audit-21504.md) corrected five omissions: one single-line function declaration in `xmlexports.h` and four array variables. The catalogue retains both sides of conditional APIs. The audited build exposes 1,367 functions and 22 variables; all 1,389 names are defined with public visibility. Its 119 additional private-external symbols are not public API additions. A controlled header-feature view covers disabled declarations without claiming those implementations are present in the built library.

The TSV does not encode configured availability, full signatures, evaluated constants or ABI layouts. `xmlversion.h.in` is the version/configuration template; each build's configured `xmlversion.h` needs separate verification. This audit covers one arm64 macOS static build and controlled header feature flags, not every platform, option combination or shared-library export surface. Historical releases and removed APIs absent from these headers need separate compatibility inventories if targeted. Complete C source or binary compatibility remains unimplemented; the pinned headers are authoritative for signatures.

The canonical identifier checksum is `0dff01385dba45acbd7d42eff82b6d5d65048e156807c5bdbd4b87024361ae43`. To reproduce it from the TSV, omit `empty_header` rows, render each row as `header<TAB>identifier<TAB>kind<TAB>marker`, where marker is `1` for `yes` and `0` otherwise, sort the complete strings by Unicode code point, join with LF, append one LF, and compute SHA-256 over UTF-8. This verifies row integrity; it does not prove semantic completeness.

The table below now hashes raw release-header bytes from the checksum-pinned `libxml2-2.15.4.tar.xz` archive, SHA-256 `98087fd181d9070724f3fbc65c7377db03038eb92bd882374daff44940138821`. It replaces the original browser file-content-view hashes, none of which matched raw release bytes; those view hashes could not verify an extracted source tree. To rerun, verify the archive and these 46 raw files, then perform the configured/header-feature audit. A maintained cross-configuration AST generator remains follow-up tooling work.

```text
header sha256
parser.h 3062aaa97d435de0afc34630162c0b3c8a994e76854a92a0e3a217b6297545e2
HTMLparser.h c4f81e8cbd194fbd4c121db47ff0eadf4b3efa9d0c780300787fb0e08372ffa4
HTMLtree.h d2280b971a031a5ed2f7f8ecff4906a9df6c3f35cb40c588eb05d60e24d8fdcb
SAX.h 777fc2057fd2a39a6ca7648ee5f148014a65643e750cc5a2fe2e2fc3b4a0ff79
SAX2.h d3228dc9e18dec65660f7ff43eb5af2cb7a9e6dad00369f76f058300c0cb1b37
c14n.h c9adaf74b4d5c0f5e23e06deb9194e7d595142914a8c36f5e725010a37e6a7c6
catalog.h 8418816149bed7463eb13fd6d260354a91f1819d2180685dc8d285181932b9c5
chvalid.h 00acfc327dc00dfb2dd640ab4e1156effd8f5fb8311f0c20d2707440456dc7ce
debugXML.h 1380bb85a7dccdbfebb1cc9ba9921873e6be11cbd4b0813824754feaa5aa5820
dict.h 12302fd488f2b735c791f2b1d9fa5b2008dd5e40f94da12e56da0aebea6e1321
encoding.h 0eabe928a57824d52b95daba1ee3e663f004d8622800f8c1f40177ea15a8a74c
entities.h ff61a6eedb81b52ad7e6bd5c005a705c871587c68314d94f7a3511f4afff8739
globals.h 4e4a9866916b928b201d0ff4a544d90098c6b639da50ca25cc9c7ce395c26349
hash.h ee8ba2adce87d740581d6763e783828b622ea8da93a5e9ac4ff36436f607bc75
list.h ba1174b44be729cde0eb443180968def8a2879db54199af670e965d331648912
nanoftp.h b902e60279e35a4818cf1834a04e654a9cf51b5136f785e89179047304cc52ef
nanohttp.h 8000d0196f6e6defd5e2468118cf3b534b15da7818b7db18167259ed857d4b7e
parserInternals.h 543da365c6c72699926ff11c0f4afa6c1990f028d7b8fcaaa30de2f284ae0718
pattern.h 169dca568e076cba001c9a445b22fb73ffa64c9699fd785abcfaf31a8e22337a
relaxng.h 7d46056b24c572f8a656637cb287fe462e880c7a1fbcd32d5ca7e618f838564c
schemasInternals.h e05a6496f4ca8e5017a94812b25bba7856110d7d85d91590ed50780ea6d98cdf
schematron.h a3d96d68d801f984b0eaab68d2b193b571ec3b27ba50bbc1c66d3f4dca187f0e
threads.h 8cee9610d9e3ec2146af6a074a2c6161bd87629e55ab231e61c0fbd8686142d2
tree.h 29bb46867633b5da9ac289f72f8667863a14b7105843f8fa2b16df21b26d0496
uri.h f761d9ac289b051ec7bbc73ae723468fa80f96461dc664fe5781bc009a92ac90
valid.h e51b4ca60f3f7921b3be0e78fe750f202b9d0a47fafc0750a806d665239f1fb9
xinclude.h 54df174ccb9f272aa5483b9c18c83153ca6bbea93e706453240ad09381fad037
xlink.h 6b0495de5f706d571995cde9e41ae6673d906a53899f7cae2061108ddad00d39
xmlIO.h e623b00dcabbf5c559567fcea561cc97f03b9fb3c8642d9edcaf70ade988385d
xmlautomata.h 41eeba10a1dc5f727694618a5513ffef4b1dcb0c27169d91b264e54eedad7d08
xmlerror.h ff6fb62996e5478f08ef99258c4f49b1a0830f93565ff3a92f47a316f792202f
xmlexports.h 36f6cc28976fed8b6c8587fd521f14ee1f4b96615f10e9b7c35c76ef2876e525
xmlmemory.h b8a8addf9376aa9bee7d3f5b6f8b58d2af4c2c284921bd418da5b27b70fe1085
xmlmodule.h ed0d48e89b3f1aac1618d4f74e87ee64bde0a8b4c7057fa56a03b56703af67cb
xmlreader.h 11fd34156675d65c73bfa83c69c8dda3d87a56553df92a5ab713bd95ab525c16
xmlregexp.h 7df116616ee4319f0654ce00b467bc165dbaf38ed318b1193d4a474a73ee1298
xmlsave.h fb99635f075e4d1c2fe858b16ef0d8853839c5df971f5dec54dcb29b872ccbbe
xmlschemas.h 0e4eeabaa87ede8e7deb65e25078bfd0caa1ddfe5c7c9a98a043f2b870fa10fd
xmlschemastypes.h 9c9b26c8badbcbf12e580ad301bc261051a4b861a9cf5801b79585b218886622
xmlstring.h 479db08576ff1a61dee723ca0285893e19d806a23c1c287f75be5242f22e8aaa
xmlunicode.h 8a5b06d87968144cdb77908a4d6c3f8b04012a26f0cf64b186869d240a696f7e
xmlversion.h.in d04e84d0b26bded0e033d17758ba7bcadbfe13721eeb2c94e5cbd9f1c8ad2ec7
xmlwriter.h 9cf89ac9e8485869db835ed25643fe58a0dbb7ee30f9191d6c400ffd7c1405d9
xpath.h 5f6392d4a0fa7fe97407738baf692a99be31218cb786cc2225d28ea66c413f9c
xpathInternals.h 4daa466e29c75e91e70556911278542c65ac3168c7ce0287ccb566bf0ce58807
xpointer.h f39e26a9380e0035c3f1c26f0a5ae2bc84ec7d1f2a172af809541c74eb1697b6
```
