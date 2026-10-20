# Remaining work

Everything between the 0.1 release and "complete": libxml2 behavior, performance on every workload, fuzz coverage, and bindings. Status as of 2026-10-20. The numbered items in [roadmap.md](roadmap.md#remaining-gaps-2026-10-19) give the history and design notes.

Size: **S** is under a week, **M** a few weeks, **L** a quarter or more. Priority: **P1** blocks a common migration off libxml2, **P2** blocks a less common one or weakens a stated goal, **P3** is rare or cosmetic.

## libxml2 behavior

| # | Gap | Size | Priority | Notes |
| --- | --- | --- | --- | --- |
| 1 | Encodings libxml2 reads only through iconv or ICU (Shift_JIS, EUC-JP, ISO-2022-JP, GBK, Big5, EBCDIC, UCS-2, UCS-4) | M | P2 | The owner chose low priority. Two parts: caller-supplied converters (`xmlRegisterCharEncodingHandler`, `xmlCtxtSetCharEncConvImpl`), which keep the core table-free, and built-in UCS-2, UCS-4 and EBCDIC. Testing needs a second oracle built with iconv. 50 `encoding.h` rows. |
| 2 | UTF-16 and single-byte input on the borrowed `Reader` and `Document` | M | P1 | `OwnedDocument`, `PushParser`, `BufferedReader` and `TextReader` decode these already; the borrowed surfaces take UTF-8 only. This accounts for 44 XMLTS gaps. |
| 3 | `RECOVER` and `NOBLANKS` on `PushParser`, `BufferedReader` and `TextReader` | M | P1 | lxml-style recovery over streams. Both DOMs have it. |
| 4 | Recovery fidelity on the DOMs | M | P2 | 82.22% of libxml2's `test/errors` and 87.12% of XMLTS not-wf trees match `xmllint --recover`. The rest are distinct repair strategies, listed in [compatibility.md](compatibility.md#recovery-recover). |
| 5 | ~~Fragment and in-context XML parsing~~ **Done 2026-10-20** | — | — | `Editor::parse_content` ([owned-dom.md](owned-dom.md#parsing-content-in-context)). Left: a text declaration (`hasTextDecl`) and input in the document's declared encoding. |
| 6 | SAX2 handler table (`xmlSAXHandler`, `xmlSAX2*`) | L | P2 | Events carry the data; this is the callback-table shape that SAX ports and lxml's target parser expect. |
| 7 | XML 1.1 and namespace-unaware parsing | L | P3 | 611 XMLTS cases per surface are unscored, plus 9 namespace-off cases. |
| 8 | DTD construction and editing (`xmlAddEntity`, `xmlAddElementDecl`, `xmlCreateIntSubset`, `xmlIsID`, `xmlIsRef`), DOCTYPE copies and edits | S–M | P2 | Read-only views exist (`valid::Dtd`). |
| 9 | Writer and reader leftovers | S | P2 | `xmlNewTextWriterDoc`/`Tree`/`PushParser`; `xmlReaderWalker` (a `TextReader` over an existing tree); `TextReader` encoding, version, quote char, `NextSibling` and `IsNamespaceDecl` accessors. `CurrentNode`/`Preserve` are declined, since `expand` returns an owned copy. |
| 10 | Error-callback model (`xmlCtxtSetErrorHandler`, `xmlGetLastError`) | S | P3 | Errors are returned instead. Known detail gaps: libxml2's column lag after entity values, and context lines for errors inside entities. |
| 11 | Diagnostic wording | S | P3 | The IDREF line in DTD stream validation, and the tree content-model "expecting" list. |
| 12 | Utility headers (`xmlunicode.h`, `xmlstring.h`, `debugXML.h` dumps) | S | P3 | `std` covers most of them. |
| 33 | Namespace errors are fatal by default | M | P1 | libxml2 reports an unbound prefix (and other `XML_NS_ERR_*` errors) as a non-fatal namespace error and keeps the node with no namespace: `xmllint` accepts `<r><p:c/></r>` with exit 0. yaxml rejects it unless `RECOVER` is set, on every surface; this is also about 6% of the in-context parsing mismatches. Matching libxml2 means keeping those errors on the document and stream like recovered errors. |
| 13 | Remaining API inventory | — | — | [libxml2-api.tsv](libxml2-api.tsv): 936 implemented, 506 partial and 1,247 not started, out of 4,060 identifiers. No P0 row is not started. Most of the not-started rows are libxml2 error-code constants (`xmlerror.h`), the schema component model (`schemasInternals.h`), SAX2 and the items above. |

Declined, with reasons recorded in the linked docs: `xmlRelaxNGDump`, `xmlCatalogConvert`, the XSD stream `ID`/`ENTITY` and counter quirks (libxml2 validity bugs), `xmlSaveSetEscape`, compressed input (`UNZIP`), FTP and HTTP.

## Performance

Measured on an Apple M1 Max only ([benchmarks.md](benchmarks.md)).

| # | Gap | Size | Notes |
| --- | --- | --- | --- |
| 14 | Small-chunk push: `namespaces` push/7 at 0.98 and `mixed` push/7 at 0.94 (libxml2 / yaxml) | S–M | The only 2 of 65 workloads where yaxml loses. Per-chunk overhead is the cost. |
| 15 | DTD-heavy documents: yaxml leads on 9 of 25 rows | M–L | `BufferedReader` is at 0.40–0.88 because of the per-event layering. DOM/owned with nested entity references is at 0.44–0.72, because yaxml re-parses each reference where libxml2 copies a cached node list. Caching parsed entity content means working through namespace, ID and error semantics. |
| 16 | `pe_chain`: 5.5% more instructions since the DTD work | S | A long chain of parameter-entity references in the internal subset. |
| 17 | Benchmarks on Linux x86-64 and in CI | S | Every number so far is from one machine. |
| 18 | Binding overhead | M | Python tree iteration is 2.5–4× slower than lxml (proxy objects), and DTD parsing is slower than lxml. The Go streaming reader is slower than `encoding/xml` (one cgo call per event; needs batching). |

## Security and fuzzing

| # | Gap | Size | Notes |
| --- | --- | --- | --- |
| 19 | Fuzz coverage: 97.58% regions, 98.75% lines, 95.04% branches | M | Most of what is left is allocation-failure arms. Reaching them needs a failing allocator. The `allocator_api` feature makes that possible for `OwnedDocument<A>` in the harness repo, where `unsafe impl Allocator` is allowed. |
| 20 | Open fuzz findings (4 inputs in `yaxml-harness/fuzz/open`) | S | A RECOVER fixed-point difference with PIs in an ATTLIST; a defaulted empty-prefix binding under RECOVER; two billion-laughs inputs that take about 2 s under ASan (0.16 s in release). They stay bounded, but one surface may be superlinear. |
| 21 | Sustained fuzzing | S | OSS-Fuzz or ClusterFuzzLite onboarding. Today, campaigns run by hand. |
| 22 | Independent security review | M | An external audit of the parser, DTD and entity handling, and the budgets before 1.0. |
| 23 | Error-site codes instead of the diagnostic re-reader | M | `src/diagnostic/reread.rs` re-parses a failing construct to find libxml2's error code. Setting the code at each error site would delete about 1,000–1,200 lines, about 55–65% of that file, and shrink the attack surface. `Error` grows by a private field. |

## Bindings

| # | Gap | Size | Notes |
| --- | --- | --- | --- |
| 24 | Publishing | M | Python wheels (maturin; CI matrix incl. free-threaded), a tagged Go module, and C ABI release artifacts (header, static and dynamic libraries, pkg-config). |
| 25 | C ABI coverage | M | Not exposed yet: resolver and XPath-extension-function callbacks, `TextReader`, `TextWriter`, HTML. |
| 26 | Node.js / TypeScript (napi-rs) | L | Not started. |
| 27 | Ruby | M | Not started. System Ruby 2.6 is too old for magnus, so this likely goes through the C ABI with the `ffi` gem. |
| 28 | Python fidelity | S | `tostring` matches lxml on 97.89% of inputs; trees match on 100%. |

## Release and project infrastructure

| # | Gap | Size | Notes |
| --- | --- | --- | --- |
| 29 | CI on a hosted runner | S | The workflows exist (Linux only) but have never run. Add macOS and Windows; Windows is untested. |
| 30 | API review before 1.0 | M | Audit `#[non_exhaustive]` use, public fields (`Error`, `Diagnostic`), naming consistency across the six parsing surfaces, and which libxml2-numbered constants should be enums. Known candidates: remove `options::LEGACY`; hide `LineIndex::describe_counted` (public but `#[doc(hidden)]`, for tests) behind a feature; `save::to_writer` with `AS_HTML` on a borrowed node reports `UnsupportedOption` where a node-type error fits better. |
| 31 | MSRV policy | S | 1.97, pinned because rustc 1.95–1.96 miscompile a struct-update pattern ([rust-lang/rust#158206](https://github.com/rust-lang/rust/issues/158206)). Relax it once fixed compilers age out. `tooling/simplify_comparison_scan.py` guards the pattern. |
| 32 | Changelog and semver policy | S | Neither exists yet. |
