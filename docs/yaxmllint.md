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
| Input | `FILE...`, `-` (stdin), `--html`, `--nodefdtd`, `--stream`, `--huge`, `--max-ampl N`, `--maxmem BYTES` |
| Output | `--noout`, `--output FILE` / `-o`, `--format`, `--pretty 0\|1\|2`, `--encode ENC`, `--xmlout`, `--noblanks`, `XMLLINT_INDENT` |
| Canonical XML | `--c14n`, `--c14n11`, `--exc-c14n` (all with comments, like xmllint) |
| XPath | `--xpath EXPR`, `--xpath0 EXPR` (implies `--noout`) |
| DTD | `--valid`, `--postvalid`, `--dtdvalid FILE`, `--loaddtd`, `--dtdattr`, `--noent` |
| Schemas | `--schematron FILE`, `--relaxng FILE`, `--schema FILE` |
| XInclude | `--xinclude`, `--noxincludenode`, `--nofixup-base-uris` |
| Resources | `--path DIRS`, `--catalogs` (reads `XML_CATALOG_FILES`), `--nocatalogs`, `--nonet` |
| Misc | `--quiet`, `--timing`, `--version` |

`--push --memory --nowarning --pedantic --nodict --nocompact --repeat --noenc
--oldxml10 --strict-namespace --load-trace` are accepted and ignored.
`--debug --shell --copy --recover --nocdata --nsclean --sax --sax1 --insert
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
not validate`) go to stderr, as in xmllint. Diagnostics read
`file:LINE: parser error : Kind` followed by the source line and a caret; the
line and column come from the error's byte offset, and the wording is yaxml's.

## Behavior notes

- Resources load from disk only: relative to the current directory, then each
  `--path` directory by file name. Other URI schemes are declined, so
  `--nonet` has nothing to do. Without `--noent`, `--loaddtd`, `--dtdattr`,
  `--valid`, `--postvalid`, `--schematron`, `--relaxng`, `--schema` or a canonical-form option
  no external subset or entity loads, as in xmllint.
- `--format` and `--noblanks` drop whitespace-only text after parsing with
  libxml2's `areBlanks` rules (declared element content, else the sibling
  heuristic; `xml:space="preserve"` respected). `--format` implies it.
- `--maxmem` bounds input and owned bytes rather than the allocator.
  `--huge` raises depth and token ceilings.
- `--stream` walks the whole document through `TextReader` and prints nothing;
  schema and DTD checks need the tree and do not run.
- A missing DTD under `--valid` is reported but not fatal, as in xmllint;
  under `--postvalid` it fails.

## Differences from xmllint

yaxml always substitutes entity references, so output of documents that use
declared entities differs from xmllint without `--noent`. It also treats an
undeclared namespace prefix, a redeclared predefined entity, trailing content
and unsupported encodings (ISO-8859-5) as fatal where libxml2 recovers.
`tests/yaxmllint.rs` runs both programs over libxml2's own test inputs when
`YAXML_XMLLINT` names the pinned binary (`YAXML_LIBXML2_SRC` its source tree)
and compares stdout and exit code: `--noout`, `--valid`, `--postvalid`,
`--c14n`, `--c14n11`, `--exc-c14n`, `--html`, `--relaxng`, `--schema` and
`--xpath` agree on every input outside the listed gaps (118/118, 118/118,
31/31, 7/7, 28/28, 21/21, 44/44, 798/798, 242/242, 327/327); default and
`--format` output agree on about 73% of all inputs, the rest being entity
references and the gaps above. The pinned `xmllint` lacks Schematron;
`--schematron` (text report, quiet under `--noout`) agrees with a
schematron-enabled build on all 84 corpus runs ([schematron.md](schematron.md)).
