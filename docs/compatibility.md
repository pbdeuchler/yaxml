# Compatibility catalogue and implementation order

This is a work inventory, not a claim of implemented compatibility. The baseline is **libxml2 2.15.4**, released 2026-09-04. Pin a source release, build options, and option combinations for every differential test; libxml2 behavior and available symbols vary by release and configuration. The [official release directory](https://download.gnome.org/sources/libxml2/2.15/) identifies this release. The [tagged headers](https://github.com/GNOME/libxml2/tree/v2.15.4/include/libxml) define the source API.

The adjacent `libxml2-api.tsv` is the mechanical identifier inventory. This document groups the required behavior and orders implementation. Nothing below is implicitly out of scope because it is low priority.

## Meaning of replacement

1. **Behavioral replacement:** equivalent XML results, events, mutations, queries, serialization, validation, and failure classification through a safe Rust API.
2. **Source migration:** familiar operations and option mappings with explicit ownership and errors. C function names are a migration index, not the recommended Rust interface.
3. **Binary replacement:** exported C symbols, ABI layouts, ownership, callbacks, global variables, allocator hooks, variadic functions, and direct mutable struct fields. This cannot be implemented faithfully by a crate that forbids unsafe: arbitrary foreign pointers cannot be dereferenced safely. Treat this as a separate future adapter with its own policy decision, not an achieved property of the safe core.

Python, Ruby, JavaScript/TypeScript, and Go bindings should target owned document handles and document-scoped node identifiers. Foreign references must keep their document alive; deletion must reject stale handles. Avoid exposing the internal tree layout. WebAssembly with copied inputs/results offers one binding route without requiring unsafe Rust in this crate. Native runtime adapters ordinarily need dependencies and unsafe foreign interfaces outside the core.

## Prioritized feature map

Priority is dependency order and migration value. P0 is the first usable secure XML subset; P1 completes ordinary XML tools; P2 enables document-processing stacks; P3 covers specialized and legacy clients.

| Priority | Feature and behavior to implement | Upstream API families / headers |
| --- | --- | --- |
| P0 | XML 1.0 strict well-formedness: declarations, names and Unicode name ranges, one root, nesting, comments, PI, CDATA, text, attributes, empty tags, legal characters, EOF and trailing content | `parser.h`, `parserInternals.h`, `chvalid.h`, `xmlstring.h`; `xmlRead*`, `xmlCtxtRead*`, `xmlCtxtParseDocument`, `xmlCtxtParseContent` |
| P0 | Entity/reference handling: five predefined entities, decimal/hex numeric references, attribute-value normalization, CR/CRLF normalization, character validity after decoding, rejection of undeclared entities | `entities.h`, parser SAX callbacks |
| P0 | Namespaces: scoped bindings, expanded names, prefix rebinding, default namespaces, reserved prefixes/URIs, duplicate expanded attribute names; preserve lexical prefixes and declaration order | `tree.h`, `SAX2.h`; `xmlNewNs`, `xmlSearchNs*`, `xmlReconciliateNs` |
| P0 | Streaming: pull events, push chunks, EOF distinct from need-more-input, stop/cancel, bounded buffering, every byte boundary including UTF-8 and delimiter splits | `xmlreader.h`, `parser.h`, `SAX2.h`; `xmlReaderFor*`, `xmlTextReaderRead`, `xmlCreatePushParserCtxt`, `xmlParseChunk` |
| P0 | Read-only DOM: document/element/attribute/text/comment/CDATA/PI identity, parent/sibling/child traversal, namespace and attribute lookup, text-content concatenation, source positions | `tree.h`; `xmlDoc`, `xmlNode`, `xmlAttr`, `xmlNs`, `xmlDocGetRootElement`, `xmlGetProp`, `xmlNodeGetContent` |
| P0 | Errors and security budgets: structured domains/codes/severity, byte and line/column positions, limits on input/token size, depth, nodes, attributes, allocation and work; no implicit external-resource access | `xmlerror.h`, `xmlCtxtSetErrorHandler`, `xmlCtxtSetResourceLoader`, `xmlCtxtSetMaxAmplification` |
| P1 | Encoding: BOM/declaration/explicit-override precedence; UTF-8/UTF-16 LE/BE, ASCII; incremental decoder and original-byte offsets; then remaining upstream built-ins and caller-provided converters | `encoding.h`; `xmlCharEncoding`, `xmlCreateCharEncodingHandler`, `xmlCtxtSetCharEncConvImpl` |
| P1 | Full DTD syntax and non-validating semantics: internal/external subsets, parameter/general entities, declarations, notations, unparsed entities, default/fixed attributes, ID typing, standalone constraints, entity-reference preservation vs substitution | `entities.h`, `valid.h`, `parser.h`, `SAX2.h`; `xmlParseDTD`, `xmlGetDocEntity`, `xmlGetID` |
| P1 | Mutable DOM: creation, insertion, unlink/remove, replacement, attributes, namespaces, copy/import/adopt, cross-document ownership, adjacent-text merging, base URI, `xml:lang`, `xml:space`, line metadata | `tree.h`; `xmlAddChild*`, `xmlAddSibling`, `xmlUnlinkNode`, `xmlReplaceNode`, `xmlSetProp`, `xmlDocCopyNode`, `xmlDOMWrap*` |
| P1 | Serialization and writer: escaping, declarations, encoding, empty-element spelling, CDATA boundaries, namespace fixup, formatting, incremental writes and I/O failures; preserve semantic round trips | `xmlsave.h`, `xmlwriter.h`, `tree.h`; `xmlSaveTo*`, `xmlSaveDoc`, `xmlTextWriter*`, `xmlDocDump*` |
| P1 | Rust I/O and contexts: slices, strings, `Read`/`BufRead`, `Write`, context reuse/reset, explicit resource resolver, deterministic cleanup, parser options and capability reporting | `xmlIO.h`, `parser.h`; `xmlReadMemory`, `xmlReadIO`, `xmlCtxtReset`, `xmlCtxtSetOptions`, `xmlHasFeature` |
| P1 | DTD validation: content models, attribute types, defaults/fixed values, required attributes, IDs/IDREFs, entities and notations; separate well-formedness from validity | `valid.h`, `xmlautomata.h`, `xmlregexp.h`; `xmlValidateDocument`, `xmlValidateDtd`, `xmlValidCtxt` |
| P2 | XPath 1.0: parser/compiler/evaluator, all axes, predicates, document order/deduplication, namespace nodes, four value types and conversions, core functions, variable/namespace/custom-function registration, limits | `xpath.h`, `xpathInternals.h`; `xmlXPathEvalExpression`, `xmlXPathCompile`, `xmlXPathCompiledEval`, `xmlXPathRegister*` |
| P2 | HTML parsing and serialization: libxml2's actual recovery/tree behavior, entities, implied elements, case folding, void/raw-text elements, encoding rules, push/SAX support | `HTMLparser.h`, `HTMLtree.h`; `htmlRead*`, `htmlCtxtRead*`, `htmlParseChunk`, `htmlDocDump*` |
| P2 | Canonicalization: Canonical XML 1.0/1.1 and exclusive C14N, node subsets, comments toggle, inclusive namespace prefixes, exact bytes | `c14n.h`; `xmlC14NDocDumpMemory`, `xmlC14NDocSaveTo`, `xmlC14NExecute` |
| P2 | XML Schema 1.0: parse/compile schemas, imports/includes/redefines, simple/complex types, facets, derivation, identity constraints, defaults, document/stream validation and diagnostics | `xmlschemas.h`, `schemasInternals.h`, `xmlschemastypes.h`; `xmlSchemaParse`, `xmlSchemaValidateDoc`, `xmlSchemaValidateStream`, `xmlSchemaVal*` |
| P2 | RELAX NG: XML syntax, include/externalRef, patterns, datatype libraries, document and push validation, diagnostics | `relaxng.h`; `xmlRelaxNGParse`, `xmlRelaxNGValidateDoc`, `xmlRelaxNGValidatePush*` |
| P2 | URI and resource resolution: parse/build/escape/normalize/resolve/relative URIs; base URI rules; XML and SGML catalogs, rewrite/delegate/nextCatalog, per-context resolver policy | `uri.h`, `catalog.h`; `xmlBuildURI`, `xmlParseURI`, `xmlCatalogResolve*`, `xmlACatalog*` |
| P2 | XInclude 1.0: XML/text inclusion, base/language fixup, fallback, recursive inclusion/cycle limits, resolver permissions, XPointer interaction | `xinclude.h`; `xmlXIncludeProcess*`, `xmlXIncludeNewContext` |
| P2 | Patterns and XPointer: streaming pattern matching, namespace-aware patterns; supported XPointer schemes and node selections | `pattern.h`, `xpointer.h`; `xmlPatterncompile`, `xmlPatternMatch`, `xmlStreamPush*`, `xmlXPtrEval` |
| P3 | XML recovery modes, deprecated SAX1 and parser-internal entry points: reproduce only against named option/release fixtures; malformed-input recovery is not a specification | `SAX.h`, `parserInternals.h`, deprecated `xmlParse*`, `xmlRecover*`, `xmlSAX*` |
| P3 | Schematron validation and reporting, XLink callbacks, public regex/automata/datatype helper APIs | `schematron.h`, `xlink.h`, `xmlregexp.h`, `xmlautomata.h`, `xmlschemastypes.h` |
| P3 | Public utility APIs: dictionaries/interning, hashes, lists, buffers, UTF-8/string helpers, Unicode categories/blocks, diagnostic tree dumps/shell | `dict.h`, `hash.h`, `list.h`, `tree.h`, `xmlstring.h`, `xmlunicode.h`, `debugXML.h` |
| P3 | Legacy process/thread globals, initialization/cleanup, memory debugging and callbacks, dynamic modules, removed HTTP/FTP compatibility stubs, build/version/export macros | `globals.h`, `threads.h`, `xmlmemory.h`, `xmlmodule.h`, `nanohttp.h`, `nanoftp.h`, `xmlversion.h`, `xmlexports.h` |

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

Start with the bounded UTF-8 XML scanner and pull events, then construct the read-only DOM from the same semantic parser. Add namespaces and normalization before advertising an ordinary XML parser. Make streaming chunk invariance, error determinism, limits, and panic freedom release gates from the first slice. Add encodings/DTD/mutation/output next; layer XPath, validators and inclusion over the resulting shared data model. Keep the low-level scanner internal so malformed syntax cannot bypass semantic checks through another public facade.
