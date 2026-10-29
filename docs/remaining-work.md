# Remaining work

Everything between the 0.1 release and "complete": libxml2 behavior, performance on every workload, fuzz coverage, and bindings. Status as of 2026-10-20. The numbered items in [roadmap.md](roadmap.md#remaining-gaps-2026-10-19) give the history and design notes.

Size: **S** is under a week, **M** a few weeks, **L** a quarter or more. Priority: **P1** blocks a common migration off libxml2, **P2** blocks a less common one or weakens a stated goal, **P3** is rare or cosmetic.

## libxml2 behavior

| # | Gap | Size | Priority | Notes |
| --- | --- | --- | --- | --- |
| 1 | Encodings libxml2 reads only through iconv or ICU (Shift_JIS, EUC-JP, ISO-2022-JP, GBK, Big5, UCS-2) | M | P2 | The owner chose low priority. UCS-4 and EBCDIC are done (feature `encodings`, pinned against an iconv-built xmllint). Caller converters are done 2026-10-22 (`ParseOptions::converters`, [encoding.md](encoding.md#caller-converters)), so any of these reads through a caller's converter. Left: converters for external entities, HTML and output, a global registry, and a built-in UCS-2 (detected by no signature; only a label). 50 `encoding.h` rows. |
| 2 | ~~UTF-16 and single-byte input on the borrowed `Reader` and `Document`~~ | — | — | **Done 2026-10-20.** `Transcoded` decodes once for `Document::parse_transcoded` and `Reader::transcoded` (UTF-8 is borrowed); XMLTS passes on both rose from 1,921 to 1,962, as on the other surfaces ([encoding.md](encoding.md#transcoded-input), design in [design.md](design.md#dom-ownership-and-mutation)). Left: offsets count the decoded text (`Transcoded::source_offset` maps one). |
| 3 | ~~`RECOVER` and `NOBLANKS` on `PushParser`, `BufferedReader` and `TextReader`~~ | — | — | **Done 2026-10-20.** Every stream (and the slice `Reader`) recovers as the DOMs do, with `errors()`; push and pull also drop blanks ([compatibility.md](compatibility.md#recovery-recover), [streaming.md](streaming.md#recovery-and-blanks)). The push parser's tree matches libxml2's recovered pull tree on 80.00% of `test/errors` and 86.11% of XMLTS not-wf, as the DOMs' events do (80.00%, 86.05%). |
| 4 | Recovery fidelity on the DOMs | M | P2 | 82.22% of libxml2's `test/errors` and 87.12% of XMLTS not-wf trees match `xmllint --recover`. The rest are distinct repair strategies, listed in [compatibility.md](compatibility.md#recovery-recover). |
| 5 | ~~Fragment and in-context XML parsing~~ **Done 2026-10-20** | — | — | `Editor::parse_content` ([owned-dom.md](owned-dom.md#parsing-content-in-context)). Left: a text declaration (`hasTextDecl`) and input in the document's declared encoding. |
| 6 | ~~SAX2 handler table (`xmlSAXHandler`, `xmlSAX2*`)~~ | — | — | **Done 2026-10-22.** `sax::Handler` and `sax::parse` (feature `sax`) call libxml2's SAX2 callbacks in its order from the slice reader ([compatibility.md](compatibility.md#sax2-callbacks)). The locator is done (`Handler::locate`, `sax::Locator`). Attribute values (defaults without `DTDATTR`, kept references, `&#38;`) are done. `NOBLANKS` changes nothing, as in libxml2 (its `areBlanks` needs a tree node). Left: source-order declarations. |
| 7 | XML 1.1 and namespace-unaware parsing | M | P3 | `OLD10` is done 2026-10-22 (Fourth Edition names, a fatal version other than 1.0; [compatibility.md](compatibility.md#option-compatibility)): the 313 historical-edition XMLTS cases agree with `xmllint --oldxml10`. XML 1.1 matches libxml2 2.15.4, which warns and reads it as 1.0. Left: scoring the historical cases with `OLD10` in the harness, `DTDVALID` Name/Nmtoken values under `OLD10`, and namespace-unaware parsing (9 namespace-off cases). |
| 8 | ~~DTD editing beyond additions~~ | — | — | **Done 2026-10-22.** `Editor::add_declarations`, `set_internal_subset` (replace or remove declarations), `remove_doctype` and `set_doctype_ids`; loaded external declarations survive ([compatibility.md](compatibility.md#dtd-declaration-model)). Left: detached `xmlNewDtd`/`xmlNewEntity`, and external declarations an internal one shadowed at parse time (dropped then). |
| 9 | Reader leftovers | S | P2 | `xmlReaderWalker` is done (`Walker`, with `NextSibling`/`CurrentNode`/`CurrentDoc`). `CurrentNode`/`Preserve` on the parsing reader are declined, since `expand` returns an owned copy. The text writer's DOM targets compose `TextWriter` with the parsers ([writer.md](writer.md#libxml2-text-writer-textwriter)). |
| 10 | Error-callback model (`xmlCtxtSetErrorHandler`, `xmlGetLastError`) | S | P3 | Errors are returned instead. Known detail gaps: libxml2's column lag after entity values, and context lines for errors inside entities. |
| 11 | Diagnostic wording | S | P3 | The IDREF line in DTD stream validation, and the tree content-model "expecting" list. |
| 12 | ~~Utility headers (`xmlunicode.h`, `xmlstring.h`, the `debugXML.h` shell)~~ | — | — | **Closed 2026-10-22.** `xmlstring.h` rows are not applicable (`str`/`String`/`char`), `xmlunicode.h` is empty in 2.15.4, and the `debugXML.h` dumps are done (`yaxml::debug`, feature `debug`). |
| 33 | ~~Namespace errors are fatal by default~~ | — | — | **Done 2026-10-20.** Every surface keeps them in `errors()` with or without `RECOVER` and keeps the node, its name in no namespace, as libxml2's `xmlNsErr` does ([compatibility.md](compatibility.md#namespace-errors)); exit status and output agree with `xmllint` on all 103 namespace-error inputs, with and without `--recover`. Left: namespace URI checks (`XML_WAR_NS_URI`), defaulted declarations libxml2 binds unchecked, defaulted attributes' errors without `DTDATTR`, entity content read per reference, colons in external-subset names, messages when one tag has several errors, and an errors list for the C ABI streams. |
| 13 | Remaining API inventory | — | — | [libxml2-api.tsv](libxml2-api.tsv): 1,383 implemented, 590 partial and none not started, out of 4,060 identifiers. Error codes that libxml2 2.15.4 never raises, or raises only from C-API guards no input reaches, count as not applicable. The IDREF table types are reachable only through functions deprecated in 2.15.4, so they count as removed upstream. |

Declined, with reasons recorded in the linked docs: `xmlRelaxNGDump`, `xmlCatalogConvert`, the XSD stream `ID`/`ENTITY` and counter quirks (libxml2 validity bugs), `xmlSaveSetEscape`, `OLDSAX` ([compatibility.md](compatibility.md#option-compatibility)), `xmlRegexpIsDeterminist` ([schema-types.md](schema-types.md#intentional-differences)), compressed input (`UNZIP` is accepted with no effect, as in a zlib-less libxml2), FTP and HTTP.

## Performance

Measured on an Apple M1 Max only ([benchmarks.md](benchmarks.md)).

| # | Gap | Size | Notes |
| --- | --- | --- | --- |
| 14 | ~~Small-chunk push: `namespaces` push/7 at 0.98 and `mixed` push/7 at 0.94~~ | — | **Done 2026-10-20.** 1.05 and 1.09; every small-chunk row is 2-18% faster and yaxml leads in 65 of 65 workloads ([benchmarks.md](benchmarks.md#small-chunk-push-2026-10-20-37f104e--5cc5929)). Left: per-`xmlns` binding copies (2-3 allocations per element) cap `namespaces` push at every chunk size. |
| 15 | DTD-heavy documents: yaxml leads on 9 of 25 rows | M–L | `BufferedReader` is at 0.40–0.88 because of the per-event layering. DOM/owned with nested entity references is at 0.44–0.72, because yaxml re-parses each reference where libxml2 copies a cached node list. Caching parsed entity content means working through namespace, ID and error semantics. |
| 16 | `pe_chain`: 5.5% more instructions since the DTD work | S | A long chain of parameter-entity references in the internal subset. |
| 17 | Benchmarks on Linux x86-64 and in CI | S | Every number so far is from one machine. |
| 18 | Binding overhead | M | Python tree iteration is 2.5–4× slower than lxml (proxy objects), and DTD parsing is slower than lxml. (The Go streaming reader now fetches 64 events per cgo call and is 1.26× faster than `encoding/xml`; [bindings.md](bindings.md#go). Python's iterparse runs on the core directly, with no per-event FFI call to batch.) |

## Security and fuzzing

| # | Gap | Size | Notes |
| --- | --- | --- | --- |
| 19 | Fuzz coverage: 97.82% regions, 98.53% lines, 94.57% branches (2026-10-20) | M | Allocation failure is now fuzzed (`allocfail` in fuzz/, a failing global allocator); 4 such regions remain. Of the other 989 missed regions, 905 are reachable (mostly budget-exhaustion `?` arms that need a budget to run out at one step, `TextWriter` misuse, XPath tree accessors) and 84 are unreachable with the reason recorded ([security](security.md#measured-coverage-2026-10-20)). Every subsystem now reports input-sized allocation failure; what stays infallible is bounded and listed in [security](security.md#allocation-failure). |
| 20 | ~~Open fuzz findings (`fuzz/open`)~~ | — | **Done 2026-10-20.** A parse stopped inside entity replacement text closes past the reference (child ranges held in their parent's); a declared attribute's own `<` is kept under RECOVER (was emptied, so output was no fixed point; libxml2's is); the billion-laughs units are linear (73,729 nodes from 1.6 KiB, about 0.47 s through the `dtd` driver in release; each doubling of the expansion doubles the marginal instructions) and closed. The stream validator now names an unbound-prefix attribute by its QName, as libxml2 and the tree do. Open: `push/recover-byte-charset-reference-offset.xml` (under `RECOVER` in ISO-8859-1, a recovered `InvalidReference` before byte `FF` is one byte apart between the stream and the slice reader). |
| 21 | Sustained fuzzing | S | OSS-Fuzz or ClusterFuzzLite onboarding. Today, campaigns run by hand. |
| 22 | Independent security review | M | An external audit of the parser, DTD and entity handling, and the budgets before 1.0. |
| 23 | Error-site codes instead of the diagnostic re-reader | M | `src/diagnostic/reread.rs` re-parses a failing construct to find libxml2's error code. Setting the code at each error site would delete about 1,000–1,200 lines, about 55–65% of that file, and shrink the attack surface. `Error` grows by a private field. |
| 34 | ~~XML Schema content-model memory~~ | — | **Done 2026-10-20.** Transitions shrank from about 150 to 44 bytes and `xsd::Limits::max_model_bytes` (64 MiB) bounds what content-model compilation holds: the 150 KiB schema of 4,000 optional elements fails within 115 MB resident instead of 1 GB, and 1,741 still compile ([security.md](security.md#allocation-failure)). libxml2 is quadratic here too and accepts 4,000 in 170 s and 468 MB. |

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
| 29 | CI on a hosted runner | S | ci.yml and harness.yml run on ubuntu-latest for every push and pull request. Add macOS and Windows runners and the per-feature combos; Windows is untested. |
| 30 | API review before 1.0 | M | Audit `#[non_exhaustive]` use, public fields (`Error`, `Diagnostic`), naming consistency across the six parsing surfaces, and which libxml2-numbered constants should be enums. Known candidates: remove `options::LEGACY`; hide `LineIndex::describe_counted` (public but `#[doc(hidden)]`, for tests) behind a feature; `save::to_writer` with `AS_HTML` on a borrowed node reports `UnsupportedOption` where a node-type error fits better. |
| 31 | MSRV policy | S | 1.97, pinned because rustc 1.95–1.96 miscompile a struct-update pattern ([rust-lang/rust#158206](https://github.com/rust-lang/rust/issues/158206)). Relax it once fixed compilers age out. `tooling/simplify_comparison_scan.py` guards the pattern. |
| 32 | Changelog and semver policy | S | Neither exists yet. |
