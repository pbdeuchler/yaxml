# yaxmllint

`yaxmllint` is `xmllint` for yaxml: the same options, exit codes and stdout
for the features yaxml implements, so scripts can migrate and the CLI can be
differentially tested against libxml2 2.15.4's `xmllint`. It adds no
dependency; arguments are parsed by hand from one option table
(`src/bin/yaxmllint/opts.rs`).

```text
cargo run --bin yaxmllint -- --format --xpath '//item[@id]' doc.xml
```

## Options

| Family | Options |
| --- | --- |
| Input | `FILE...`, `-` (stdin), `--recover`, `--html`, `--nodefdtd`, `--stream`, `--huge`, `--max-ampl N`, `--maxmem BYTES`, `--nocdata`, `--nsclean`, `--noenc` |
| Output | `--noout`, `--output FILE` / `-o`, `--format`, `--pretty 0\|1\|2`, `--encode ENC`, `--xmlout`, `--noblanks`, `XMLLINT_INDENT` |
| Canonical XML | `--c14n`, `--c14n11`, `--exc-c14n` (all with comments, like xmllint) |
| XPath | `--xpath EXPR`, `--xpath0 EXPR` (implies `--noout`) |
| DTD | `--valid`, `--postvalid`, `--dtdvalid FILE`, `--loaddtd`, `--dtdattr`, `--noent`, `--oldxml10` |
| Schemas | `--schematron FILE`, `--relaxng FILE`, `--schema FILE` |
| XInclude | `--xinclude`, `--noxincludenode`, `--nofixup-base-uris` |
| Resources | `--path DIRS`, `--catalogs` (reads `XML_CATALOG_FILES`), `--nocatalogs`, `--nonet` |
| Misc | `--quiet`, `--timing`, `--version` |

`--push --memory --nowarning --pedantic --nodict --nocompact --repeat
--strict-namespace --load-trace` are accepted and ignored.
`--debug --shell --copy --sax --sax1 --insert
--walker --dropdtd --auto --compress --pattern --dtdvalidfpi` are
xmllint options with no counterpart: they print `unsupported option` and exit
1. Unknown options print the usage and exit 1.

## Exit codes

As `XMLLINT_ERR_*`: 0 ok, 1 unclassified (usage, XInclude failure, `--stream`
parse failure), 2 DTD could not be parsed, 3 validation failed, 4 not
well-formed or unreadable, 5 schema did not compile, 6 output or
canonicalization failed, 9 `--maxmem` exceeded, 10 XPath error, 11 XPath
node-set empty. With several files the last failure wins.

Verdict lines (`FILE validates`, `FILE fails to validate`, `Document FILE does
not validate`) go to stderr, as in xmllint. Parse errors and validity
diagnostics are libxml2's: `Error::describe` (or `LineIndex::describe_all`
under `--recover`) and the validators' `describe`, printed by
`Diagnostic::render` (`FILE:LINE: [element NAME: ]DOMAIN error : MESSAGE`,
then for parse errors the context line and caret;
[bindings.md](bindings.md#error-mapping)). yaxml stops at the first parse
error where libxml2 goes on reporting, so later lines differ.
`parse_errors_against_xmllint` compares stderr with xmllint's over
libxml2's `test/errors` and the XMLTS not-wf documents: the first error line
agrees on 38/41 (92.68%) and 1063/1065 (99.81%), that error's report with
its context lines on 38/41 and 1059/1065 (99.44%), the whole stderr on 22/41
(53.66%) and 454/1065 (42.63%); before, with yaxml's own wording, none
agreed. Validity lines (`--postvalid` over `test/valid` and `test/VC`,
`--relaxng` and `--schema` over their corpora) agree on 47 of 80 runs
(30 before), where the validators' messages differ.
`--maxmem` failures print xmllint's `Maximum memory exceeded (N bytes)`.

## Behavior notes

- Resources load from disk only: relative to the current directory, then each
  `--path` directory by file name. Other URI schemes are declined, so
  `--nonet` has nothing to do. What loads follows xmllint's parser options:
  the external subset with `--loaddtd`, `--dtdattr`, `--valid`,
  `--postvalid`, `--dtdvalid`, `--schematron`, `--relaxng`, `--schema` or a
  canonical form; external entities with `--noent` (plus a subset option),
  `--valid`, or the three schema options, which add `NOENT` as xmllint does.
  `--noent` alone reads no external subset, as in xmllint.
- `--format` and `--noblanks` parse with `NOBLANKS`: libxml2's `areBlanks`
  rules (declared element content, else the sibling heuristic;
  `xml:space="preserve"` respected; the heuristic keeps text before an entity
  reference).
  `--format` implies it.
- `--maxmem` bounds input and owned bytes rather than the allocator.
  `--huge` sets `HUGE` and raises the owned-byte budget.
- `--stream` walks the whole document through `TextReader` and prints nothing
  but `--valid`, `--relaxng` and `--schema` results: the reader validates as it
  reads, printing the diagnostics, `Document FILE does not validate` or one schema verdict, then
  `failed to parse` if reading failed, as xmllint does. Without a DTD
  `--stream --valid` fails (exit 3), as in xmllint; content-model errors are
  reported once each ([textreader.md](textreader.md#dtd-validation)).
  Schematron and `--postvalid` need the tree and do not run.
- A missing DTD under `--valid` is reported but not fatal, as in xmllint;
  under `--postvalid` it fails.
- `--recover` parses with `RECOVER`: each recovered error is printed as a
  parser error and the recovered document is output with exit code 0, as in
  xmllint, also one from which no element is recovered (only empty input
  fails, exit 4).
  `--stream` ignores it, as `TextReader` does not recover.

## Differences from xmllint

Options map to the same `ParseOptions` flags as xmllint's (`--noent`,
`--loaddtd`, `--dtdattr`, `--valid`, `--noblanks`, `--nocdata`, `--nsclean`,
`--noenc`, `--huge`, and the loading `--postvalid`, `--dtdvalid`,
`--relaxng`, `--schema` and canonical forms imply), so entity references are
kept without `--noent`, in content and attribute values (`a="&name;"`, also
in `--xpath` attribute output). An `<!ATTLIST>` default with a reference is
written substituted where libxml2 escapes the reference
([compatibility](compatibility.md#entity-references-without-noent)). Without `--recover` yaxml also treats an
undeclared namespace prefix, a redeclared predefined entity, trailing content
and unsupported encodings (ISO-8859-5) as fatal where libxml2 continues.
`tests/yaxmllint.rs` runs both programs over libxml2's own test inputs when
`YAXML_XMLLINT` names the pinned binary (`YAXML_LIBXML2_SRC` its source tree)
and compares stdout and exit code: `--noout`, `--valid`, `--postvalid`,
`--stream --valid`, `--c14n`, `--c14n11`, `--exc-c14n`, `--html`, `--relaxng`, `--stream --relaxng`,
`--stream --schema`, `--schema`, `--xpath` and default output agree on every input outside the
listed gaps (120/120, 120/120, 33/33, 120/120, 7/7, 28/28, 21/21, 44/44, 798/798, 798/798, 242/242, 242/242,
327/327, 120/120); over all inputs default and `--format` output agree on 130/143 and 127/143
(105 and 104 before entity references were kept, 127 and 126 before attribute references were),
the rest being the gaps and, for `--format`, three documents where the `NOBLANKS` sibling
heuristic differs. `attribute_references_against_xmllint` compares eight flag sets over
documents with attribute references and requires all to agree. `--recover` agrees with
`xmllint --recover` on 37/45 `test/errors` inputs (82.22%) and on 1305/1498
XMLTS not-wf documents (87.12%); the differences are listed under
[Recovery](compatibility.md#recovery-recover).
`recover_against_xmllint` reports these rates (`YAXML_SUITES` names the
directory of `xmlconf-catalog.tsv`) and requires a curated table of
recoverable inputs to agree. The pinned `xmllint` lacks
Schematron; `--schematron` (text report, quiet under `--noout`) agrees with a
schematron-enabled build on all 84 corpus runs ([schematron.md](schematron.md)).
