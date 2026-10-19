# Schematron

`yaxml::schematron` ports libxml2 2.15.4 `schematron.c`: the same subset of
ISO Schematron and Schematron 1.5, the same validation passes and the same
report text, byte for byte on libxml2's corpus. Rule contexts are
[`yaxml::pattern`](pattern.md) patterns, as in libxml2; tests, `let` values,
`name/@path` and `value-of/@select` are [`yaxml::xpath`](xpath.md) expressions.

| Call | libxml2 equivalent |
| --- | --- |
| `Schema::parse(bytes, uri, resolver, &limits)` | `xmlSchematronNewParserCtxt` + `xmlSchematronParse` |
| `Schema::from_tree(&tree, uri, source, &limits)` | `xmlSchematronNewDocParserCtxt` + `xmlSchematronParse` (either DOM) |
| `schema.validate(&tree, flags, source)` | `xmlSchematronNewValidCtxt(schema, flags)` + `xmlSchematronValidateDoc` (either DOM) |
| `OUT_QUIET`, `OUT_TEXT`, `OUT_XML`, `OUT_ERROR` | `XML_SCHEMATRON_OUT_*` |

```rust
use yaxml::schematron::{Limits, OUT_TEXT, Schema};

let sct = br#"<schema xmlns="http://purl.oclc.org/dsdl/schematron">
  <pattern id="ids"><rule context="item">
    <assert test="@id">An <name/> needs an id</assert>
  </rule></pattern></schema>"#;
let none = |_: yaxml::Request<'_>| Ok(None);
let schema = Schema::parse(sct, "s.sct", Box::new(none), &Limits::default())?;
let input = b"<list>\n<item id='a'/>\n<item/>\n</list>";
let validation = schema.validate(&yaxml::Document::parse_bytes(input)?, OUT_TEXT, input);
assert_eq!(validation.errors, Some(1));
assert_eq!(validation.render("d.xml"), "Pattern: ids\n/list/item[2] line 3: An item needs an id\n");
# Ok::<(), Box<dyn std::error::Error>>(())
```

`Schema` is immutable, `Send + Sync` and reusable. `source` is the text the
tree was parsed from; it only supplies line numbers (`&[]` gives line 0, as
libxml2 reports for nodes without one). A schema error is `SchemaErrors`:
every `Diagnostic<()>` libxml2 raises, in its order, with `Code::number()`
giving the `xmlParserErrors` value. `Diagnostic::render(file)` and
`Validation::render(file)` print exactly what libxml2 prints through
`xmlFormatError` and `xmlPrintErrorMessage`.

## Schemas

The root is `schema` in either namespace; elements in other namespaces are
skipped everywhere. Then an optional `title`, `ns` elements (prefix and URI
for contexts and expressions), and `pattern`s; anything else in the
Schematron namespace (`phase`, `include`, `diagnostics`, `let`, a late
`title`) is an error, as in libxml2, whose include support is compiled out.
A pattern is named by `id`, else `name`; one with neither is skipped unread.
A `rule` needs a `context` pattern and contains `let` (`name`, `value`),
`assert` and `report` (`test`). Messages are the text and CDATA of the
assert or report plus `name` (`path`, default the context node) and
`value-of` (`select`); other children are dropped, and trailing whitespace
after each part collapses to one space. Every error message, code and
order is libxml2's ("rule has no context attribute", "Failed to compile test
expression ...", "assert has an empty test attribute" also for reports, and
so on). Expressions compile as `xmlXPathCtxtCompile` with
`XML_XPATH_CHECKNS`, so an unbound name-test prefix fails the schema, and
each XPath error is reported before the Schematron one.

## Validation

Like `xmlSchematronValidateDoc`, with `OUT_QUIET` or no flags the document
is walked once and every rule of every pattern is tried at each element;
otherwise pattern by pattern, printing `Pattern: NAME` in text mode. A
failed assert or fired report counts as an error and, unless quiet hides
reports or `OUT_XML` (SVRL, never written by libxml2) is set, produces
`PATH line N: MESSAGE` as `Output::Text`, or as an `Output::Error` diagnostic
under `OUT_ERROR`. Paths are `xmlGetNodePath`'s (`/r/b[2]`, `*` for an
unprefixed name in a namespace); a message with no parts is `node failed
assert`/`node failed report`. Lines are those libxml2 records: where the
element's start tag ends. `value-of` prints numbers with `%g`, booleans as
`True`/`False`, and node-sets as the libxml2 `name` field of each member
(local names, `text`, `comment`, PI targets, namespace URIs, nothing for
CDATA and the root), separated by spaces; a failing `value-of` ends the
message.

`let` values are evaluated last first into one variable table shared by the
whole validation and removed after the rule's tests, so a `let` may only
use later ones; a failed evaluation reports `Evaluation of compiled
expression failed`, and a failed removal (a name never registered, or used
twice) reports `Unregistering a let variable failed` and leaves the rest of
the table in place for later rules. Both count as errors; in the one-pass
mode they stop validation with `errors: None`, libxml2's -1, which is also
the result for a schema without rules. A document without an element counts
one error.

## Limits

`Limits { xml, xpath, max_operations }`: XML limits for the schema
document, XPath compilation and per-expression evaluation limits, and
500 million XPath operations for one validation. When those run out the
expression fails with `Operation limit exceeded` and validation stops with
`errors: None`; libxml2 has no such budget.

## Agreement with libxml2

The pinned oracle is libxml2 2.15.4 built with
`-DLIBXML2_WITH_SCHEMATRON=ON` (off by default in 2.15) and
`tooling/libxml2_schematron.c`:

```sh
cmake -S ../yaxml-oracle/libxml2-2.15.4 -B /private/tmp/sch/build -DBUILD_SHARED_LIBS=OFF \
  -DLIBXML2_WITH_SCHEMATRON=ON -DLIBXML2_WITH_ICONV=OFF -DLIBXML2_WITH_ZLIB=OFF \
  -DLIBXML2_WITH_PYTHON=OFF && cmake --build /private/tmp/sch/build
cc -I/private/tmp/sch/build -I/private/tmp/sch/build/libxml -I../yaxml-oracle/libxml2-2.15.4/include \
  tooling/libxml2_schematron.c /private/tmp/sch/build/libxml2.a -lm -lpthread -o /private/tmp/sch/helper
YAXML_SCHEMATRON_HELPER=/private/tmp/sch/helper cargo test --test schematron -- --include-ignored
```

- libxml2's `test/schematron` (18 schemas, 42 instances, vendored in
  `tests/fixtures/schematron`): the runtest protocol (`OUT_ERROR`)
  reproduces all 42 `result/schematron/*.err` files byte for byte, over
  both DOMs, in every `cargo test`.
- The helper compares the same corpus in eight flag combinations (0,
  quiet, text, text+quiet, XML, error, error+quiet, error+text) and 43 table
  cases covering every schema error and validation path: 187 of 187 runs
  print identical output.
- `yaxmllint --schematron` against the schematron-enabled `xmllint`, with
  and without `--noout`, over the corpus: 84 of 84 runs agree on stdout,
  stderr and exit code.

## Differences

- XPath is yaxml's: core-function arity is checked when compiling, so
  `count()` fails the schema where libxml2 fails the test at validation;
  `f(` (no closing parenthesis) is a syntax error, which libxml2 accepts;
  the context position and size are 1, where libxml2 leaves them unset so
  `position()` and `last()` fail. Some compilation errors are reported with
  another kind or caret: an unexpected character is `Char out of XML range`
  and a missing `)` `Missing closing curly brace` (libxml2: `Invalid
  expression`), and an unfinished literal or a bare `$` shows a caret
  libxml2 omits. Rule contexts carry the [pattern differences](pattern.md).
- `XML_XPATH_CHECKNS` applies to `from_tree` too; libxml2's document and
  memory parser contexts defer unbound prefixes to validation.
- A schema that is not well-formed gives only libxml2's `could not load`
  error, not the parser's own messages. The resolver also loads the
  schema's external subset; libxml2 parses schemas with `XML_PARSE_NOENT`,
  which loads only external entities.
- Lines count LF bytes of `source`; elements from entity replacement text
  have line 0, and lines above 65535 are not capped as libxml2 does without
  `XML_PARSE_BIG_LINES`. Messages are cut to 998 bytes on a character
  boundary rather than mid-character.
- In the one-pass `OUT_ERROR` mode, `Diagnostic::pattern` is the pattern's
  name; libxml2 passes a mistyped pointer there.
- `NaN` prints as `nan` (the macOS libc; glibc prints `-nan` for a negative
  NaN). `XML_SCHEMATRON_OUT_FILE`, `OUT_BUFFER` and `OUT_IO`, which libxml2
  ignores, are not provided; output is returned instead of printed.

## Fuzzing

`fuzz/schematron.rs` (target `schematron`) splits a testcase at NUL into
schema and instance, wrapping a schema piece that is not markup in a rule
with context `*`, and checks: `from_tree` over both DOMs reports what
`parse` does; each output mode gives identical results over both DOMs and on
reuse; text and error modes print the same messages and counts; the one-pass
count equals the error-mode count when no internal error occurred; and a
smaller operation budget either runs out or changes nothing. Seeds are the
libxml2 corpus and a few hand-written cases; the tracked corpus
(921 files of at most 4 KiB) is their edge-only merge with the campaign's
finds. On `nightly-2026-06-06`, a 669 s run (`-fork=6 -timeout=2
-max_len=4096`, 31.9 million executions, 7,909 edges) after a 313 s one found
no crash, timeout or leak. Replaying the corpus with `fuzz/coverage.sh
schematron` covers 92.42% of `src/schematron.rs` lines, 91.60% of regions
and 83.64% of branches.
