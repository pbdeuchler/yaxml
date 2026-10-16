# libxml2 2.15.4 public inventory audit

On 2026-10-16, a configured Clang AST and Mach-O static-symbol audit found five
omissions in the original 4,051-entry lexical catalogue. The corrected
[inventory](../libxml2-api.tsv) has **4,056 identifiers**, plus four unchanged
`empty_header` rows. No original row was removed or changed, including disabled
and deprecated APIs. This is catalogue validation, not implemented Rust or C ABI
compatibility.

## Corrections

| Header and source line | Identifier | Kind | Deprecated marker |
| --- | --- | --- | --- |
| [`xmlexports.h:90`](https://github.com/GNOME/libxml2/blob/v2.15.4/include/libxml/xmlexports.h#L90) | `xmlCheckVersion` | function | not_marked |
| [`chvalid.h:55`](https://github.com/GNOME/libxml2/blob/v2.15.4/include/libxml/chvalid.h#L55) | `xmlIsPubidChar_tab` | variable, 256-byte array | not_marked |
| [`parserInternals.h:252`](https://github.com/GNOME/libxml2/blob/v2.15.4/include/libxml/parserInternals.h#L252) | `xmlStringText` | variable, array | not_marked |
| [`parserInternals.h:253`](https://github.com/GNOME/libxml2/blob/v2.15.4/include/libxml/parserInternals.h#L253) | `xmlStringTextNoenc` | variable, array | not_marked |
| [`parserInternals.h:255`](https://github.com/GNOME/libxml2/blob/v2.15.4/include/libxml/parserInternals.h#L255) | `xmlStringComment` | variable, array | yes |

All five have public definitions in the audited archive. The original lexical
sweep missed the single-line function declaration in the export-macro header
and array declarators. The corrected totals are 1,405 functions, 22 variables,
86 callbacks, 202 other typedefs, 48 enums, 1,288 enumerators, 284 macros,
55 named structs and 666 member paths. No other kind/name or deprecation-marker
differences were found within the header views below. Nested member-path markers
include an enclosing deprecated member, not only the final field's direct AST
attribute.

## Inputs and toolchain

- yaxml audit base: `a8d0130`; documentation-only corrections on an isolated branch.
- Source: extracted `libxml2-2.15.4.tar.xz`, SHA-256
  `98087fd181d9070724f3fbc65c7377db03038eb92bd882374daff44940138821`.
  All 46 catalogue header/template files matched their archive bytes exactly;
  no additional public header files were present.
- Host: Darwin 27.0.0, arm64, `RELEASE_ARM64_T6000`.
- Clang: Apple 21.0.0, `clang-2100.3.34.2`, target
  `arm64-apple-darwin27.0.0`. `nm`: Apple LLVM 21.0.0. CMake: 4.3.2.
  Audit processing: Ruby 2.6.10, standard libraries only.
- Build: `BUILD_SHARED_LIBS=OFF`, `Release`, `/usr/bin/cc`, GNU C11,
  `-O3 -DNDEBUG -fPIC`, source/include and build include directories.
- Enabled: C14N, catalog, debug, HTML, modules, output, pattern, programs, push,
  reader, regexps, RELAX NG, SAX1, schemas, threads, validity, writer, XInclude,
  XPath and XPointer. Disabled: HTTP stubs, iconv, ICU, ISO8859X, legacy,
  Schematron, thread allocation hooks, TLS, zlib, Python, tests, docs and readline.

| Build artifact | SHA-256 |
| --- | --- |
| `build/libxml2.a` | `b48612c66e7d4d7fae1ceb7db9a0d3e288085bc91b560f037862607eccac1ab0` |
| `build/CMakeCache.txt` | `6ef94f82ffa02e99a2d5dfcab1f23cf4cbf68d11a6706b1df46c2949ba94d6ec` |
| `build/libxml/xmlversion.h` | `746fb768896dbde19d8230fe1c4dbad175324678013901d0ba7c96fd8fde2288` |

The current [raw-header checksum table](../compatibility.md#api-inventory-method-and-limits)
replaces the previous browser file-content-view hashes. All 46 previous values
differed from raw release bytes; they were not usable extracted-tree checksums.
The original identifier checksum reproduced before correction:
`dbfde559ca150a1505455a98024c16fdfe2955e066de90ff5a225ac636d13cba`.
The corrected canonical checksum is
`0dff01385dba45acbd7d42eff82b6d5d65048e156807c5bdbd4b87024361ae43`;
the corrected TSV file checksum is
`18a168f45a9276060fa4620b564fb83bfbf27e6964661af5245850d64a66cceb`.

## Denominators and configuration boundaries

The configured header AST has **3,718 non-macro entries**: 1,367 functions,
22 variables, 84 callbacks, 196 other typedefs, 47 enums, 1,281 enumerators,
55 structs and 666 member paths. Its **1,389 distinct function/variable names**
exactly match the archive's public-external names. All are defined; no additional
public-external name lacks a public declaration. `nm -gU` also lists **119
private-external names**; `nm -gUm` distinguishes them from the public surface.
They are not missing public inventory rows.

A second, controlled **header-only** view defines the seven disabled public
feature macros for HTTP stubs, iconv, ICU, ISO8859X, Schematron, thread allocation
hooks and zlib. It has 3,767 non-macro entries; enabling thread allocation hooks
replaces five public allocator variables with accessor declarations/macros.
The union with the actual build view is **3,772 entries**, exactly all corrected
non-macro catalogue rows. This view does not rebuild the library or assert that
its disabled implementations are available.

The actual build omits 54 catalogue declarations: 23 HTTP functions, ten
Schematron functions, five thread-allocation accessors, seven Schematron enum
values, six typedefs, one enum and two callbacks. All remain catalogued.

The configured preprocessor emits 271 catalogue macro definitions. The two
header views expose 276 distinct header/name pairs. Seven more are the disabled
feature switches supplied on the second command line, whose definitions remain
visible in the raw version template. The last row,
`XML_SCHEMAS_TYPE_REDEFINING`, is intentionally inside `#if 0` in
`schemasInternals.h:506`. These explain all **284** macro rows. This does not
audit every platform-dependent macro expansion or evaluate every constant.

## Reproduction and artifacts

Local inputs are `/private/tmp/yaxml-oracle-21504/{libxml2-2.15.4,build}`;
raw outputs and scripts are in `/private/tmp/yaxml-api-audit-21504`.
`audit.rb` generates a translation unit including all 46 headers, substituting
configured `xmlversion.h` for the template. Exact local reruns are:

```sh
ruby /private/tmp/yaxml-api-audit-21504/audit.rb \
  /private/tmp/yaxml-api-inventory-fix/docs/libxml2-api.tsv \
  /private/tmp/yaxml-api-audit-21504/corrected
ruby /private/tmp/yaxml-api-audit-21504/audit.rb \
  /private/tmp/yaxml-api-inventory-fix/docs/libxml2-api.tsv \
  /private/tmp/yaxml-api-audit-21504/corrected-feature-view \
  LIBXML_HTTP_STUBS_ENABLED LIBXML_ICONV_ENABLED LIBXML_ICU_ENABLED \
  LIBXML_ISO8859X_ENABLED LIBXML_SCHEMATRON_ENABLED \
  LIBXML_THREAD_ALLOC_ENABLED LIBXML_ZLIB_ENABLED
ruby /private/tmp/yaxml-api-audit-21504/verify.rb
```

The underlying compiler commands use the source/build paths above:

```sh
clang -std=gnu11 -DNDEBUG -I"$build" -I"$source/include" \
  -E all-public.c > all-public.i
clang -std=gnu11 -Xclang -ast-dump=json -fsyntax-only all-public.i \
  > all-public-ast.json
clang -std=gnu11 -DNDEBUG -I"$build" -I"$source/include" \
  -E -dD all-public.c > all-public-macros.i
nm -gU "$build/libxml2.a" > nm-global-defined.txt
nm -gUm "$build/libxml2.a" > nm-visibility.txt
```

For the second header view, add these preprocessor flags to the `-E` commands:

```text
-DLIBXML_HTTP_STUBS_ENABLED -DLIBXML_ICONV_ENABLED -DLIBXML_ICU_ENABLED
-DLIBXML_ISO8859X_ENABLED -DLIBXML_SCHEMATRON_ENABLED
-DLIBXML_THREAD_ALLOC_ENABLED -DLIBXML_ZLIB_ENABLED
```

The raw directory contains `audit.rb`, `verify.rb`, exact command arrays,
translation units, both preprocessed views and ASTs, normalized declarations,
macro definitions, symbol/visibility output, source hashes, original/corrected
summaries, `CMakeCache.txt`, `flags.make` and tool metadata. `checks.json` records
successful preservation, archive/header, union and public-symbol assertions;
`artifact-sha256.txt` hashes the retained files.

AST source attribution uses physical offsets in preprocessed input and its
`#line` map, avoiding omitted/reused JSON file fields. Callback classification
uses function-pointer types; only a typedef's immediately owned enum tag defines
an enum. Anonymous nested record fields become complete member paths. This avoids
misclassifying callbacks returning enums and follows the existing TSV categories.

## Remaining limits

This checks one arm64 macOS static build and selected header conditional branches.
It does not verify shared-library exports, Windows/C++ behavior, calling
conventions, ABI layouts, all constant values or every build combination. Static
private-external symbols must not be presented as an exported ABI. A maintained
cross-platform extraction tool and per-operation Rust mappings remain open.
No implementation, dependency, C ABI or performance claim changes here.
