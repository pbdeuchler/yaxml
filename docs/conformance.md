# XML conformance checks

`tests/conformance.rs` contains independently expected cases for the implemented
UTF-8, namespace-aware XML 1.0 subset. These supplement the existing reader and
streaming suites; they are neither the W3C test suite nor a claim of complete XML
conformance. They require no external oracle or additional dependencies.

The acceptance tables run through `Reader`, `Document`, and `PushParser`. Push
input uses byte chunks of sizes 1, 2, 3, 5, 8, 13, and the full input. Inputs of
at most 40 bytes additionally exercise every two-way byte split with an empty
feed between chunks. This includes splitting inside multibyte UTF-8 characters.

The tests check these independently specified properties:

| Area | Basis and expected behavior |
| --- | --- |
| Document boundaries and delimiters | [XML §§2.1, 2.4–2.8, 3.1](https://www.w3.org/TR/2008/REC-xml-20081126/): one root, restricted top-level material, exact delimiters, quoted attributes, and literal versus escaped content. |
| Declarations | [XML §2.8](https://www.w3.org/TR/2008/REC-xml-20081126/#sec-prolog-dtd): grammar for `1.` followed by ASCII digits; forward versions use XML 1.0 rules. Declaration fields retain their required order. |
| Encoding declarations | [XML §4.3.3](https://www.w3.org/TR/2008/REC-xml-20081126/#charencoding): malformed encoding names differ from syntactically valid unsupported encodings. |
| Characters and references | [XML §§2.2, 4.1](https://www.w3.org/TR/2008/REC-xml-20081126/): legal character boundaries, strict reference syntax, and single expansion. |
| Names | [XML §2.3](https://www.w3.org/TR/2008/REC-xml-20081126/#sec-common-syn) and [Namespaces §§4, 7](https://www.w3.org/TR/2009/REC-xml-names-20091208/): representative character-class boundaries in element, attribute, prefix, local-name, and PI-target positions. |
| Namespaces | [Namespaces §§3, 6](https://www.w3.org/TR/2009/REC-xml-names-20091208/): reserved bindings, exact URI identity, scope, declaration order independence, and expanded attribute uniqueness. |
| Logical values | [XML §§2.11, 3.3.3](https://www.w3.org/TR/2008/REC-xml-20081126/): explicit goldens distinguish literal line endings and attribute whitespace from character references; CDATA, comments, and PIs preserve reference spellings. Unicode composition is preserved. |

Reserved prefixes beginning with a case variant of `xml` are not fatal solely
because they are reserved. Namespace URI comparison does not perform case folding
or percent decoding. The default namespace applies to unprefixed elements and
does not apply to unprefixed attributes. These details have acceptance fixtures
and explicit expanded-name goldens.

Valid DTD documents and valid non-UTF-8 encoding declarations have a separate
unsupported-feature table for the borrowing interfaces. DTD parsing, validation, external entities,
XML 1.1 semantics, and URI-reference validation are outside this suite's scope.
Namespace processors are not required to validate namespace URI syntax
([Namespaces §8](https://www.w3.org/TR/2009/REC-xml-names-20091208/#Conformance)).
Early DTD rejection does not classify the remaining DTD grammar as well formed.

Run `cargo test --offline --test conformance`. Expected error kinds are yaxml's
public diagnostic categories; the specifications define acceptance and values,
not yaxml's exact categories or byte offsets. Fixtures avoid relying on error
precedence when multiple independent defects are present.

`tests/utf16.rs` independently checks both byte orders, every byte split,
declaration/BOM agreement, malformed code units, raw source ranges and errors,
decoded/source budgets, cancellation, panic poisoning, I/O, and source-independent
owned storage. These exercise the auto-decoding interfaces under the
[encoding contract](encoding.md); they do not expand borrowing APIs to UTF-16.

## Complete external catalog

The `xml_conformance` example runs the complete root catalog from the
[W3C XML Test Suite](https://www.w3.org/XML/Test/), release **20130923**.
It records all 2,585 cases across Reader, borrowing DOM, owned DOM, push and
buffered pull: 12,925 result rows. It does not silently omit XML 1.1, historical
editions, unsupported features, discretionary errors or unavailable inputs.
The [validation report](archive/validation-xml-conformance.md) records the exact scope,
counts, hashes, review and remaining gaps. This is a complete catalog run,
not a claim of complete XML conformance.

The suite's `testcases.dtd` defines nonvalidating behavior: both `valid` and
`invalid` cases require acceptance; supported `not-wf` cases require rejection.
`invalid` means DTD-invalid, not malformed XML. This runner scores terminal
well-formedness outcomes, without DTD validity or canonical-output comparison.
Namespace-disabled cases are capability gaps because every yaxml surface is
namespace-aware. XML/Namespaces 1.1 and editions excluding XML 1.0 Fifth Edition
are observed but unscored. External entities load through a `FileResolver`
rooted at the suite with the case path as base. Explicit unsupported encoding
errors and resource limits remain visible gaps. Worker failures
are inconclusive and fail the run.

Metadata extraction is separate from the processor under test. The optional
`harness/tooling/libxml2_xmlconf_catalog.c` executable uses the pinned libxml2 2.15.4
oracle to expand trusted catalog entities/defaults and resolve XML Base. It
emits a strict, versioned 12-column TSV catalog. No suite fixture or C helper
is linked into the Rust crate; Cargo still depends only on `memchr`.
The helper is for the checksum-pinned descriptor files, not untrusted XML.
Test inputs reach yaxml only with suite-rooted file resolution.

After building the [pinned oracle](oracle.md), compile the metadata helper:

```sh
oracle_source=/path/to/libxml2-2.15.4
oracle_build=/path/to/libxml2-build
cc -std=c11 -Wall -Wextra -Werror -pedantic \
  -I"$oracle_build" -I"$oracle_source/include" \
  tooling/libxml2_xmlconf_catalog.c "$oracle_build/libxml2.a" \
  -lm -pthread -o /tmp/xmlconf-catalog-helper
```

The archive must have SHA-256
`9b61db9f5dbffa545f4b8d78422167083a8568c59bd1129f94138f936cf6fc1f`
before extraction. This is our pinned digest of the official download, not a
claim of an upstream signature. The CI workflow contains the complete download,
checksum, extraction, helper-test and run recipe (`./oracle.sh xmlconf` at the
repository root runs it). By hand, in `harness/`, given the extracted `xmlconf`
directory:

```sh
suite_root=/absolute/path/to/xmlconf
XML_CATALOG_FILES= SGML_CATALOG_FILES= /tmp/xmlconf-catalog-helper \
  "$suite_root/xmlconf.xml" "$suite_root" > /tmp/xmlconf-catalog.tsv
cargo run --release --example xml_conformance -- \
  --manifest /tmp/xmlconf-catalog.tsv --suite-root "$suite_root" \
  --expected-cases 2585 \
  --required-pass tests/fixtures/xmlconf-20130923-required.tsv
```

CI requires the checked-in per-ID, per-interface pass baseline. A required
`pass` becoming a gap, unscored result or other failure makes the command fail;
the original result row is retained. Newly supported pairs may become passes.
`hst-lhs-007` and `hst-lhs-008` expect rejection of a BOM/label disagreement that libxml2 2.15.4 accepts with a warning; yaxml follows libxml2, so the runner scores an acceptance there as a `gap` (`libxml2-encoding-mismatch-warning`), not a mismatch.

The baseline protects 9,810 pairs across 1,962 IDs. Its masks follow
`reader, document, owned, push, buffered`. Review baseline additions alongside
new feature evidence; do not remove requirements merely to accept a regression.
Omitting `--required-pass` is useful for exploring another catalog, and the
summary explicitly reports zero required pairs.

The runner reconciles its catalog count with `--expected-cases` before executing.
Each input is at most 1 MiB; metadata is at most 4 MiB/10,000 cases, with
1,024-byte fields. A fresh child runs all five surfaces, with a combined
five-second deadline and 4 KiB combined output ceiling. Push cycles literal
1/7/4,096-byte chunks and finishes; buffered pull drains a 17-byte buffer to EOF.
A 300-second execution budget stops launching children and emits inconclusive
rows for the remainder. Filesystem operations and output backpressure are not
OS-enforced time or memory limits. Canonical path containment and regular-file
checks assume the local extracted suite is not concurrently replaced.

The upstream master catalog has nine EdUni base-directory errors. Exact ID/path
corrections map `eduni/namespaces/misc/001..009.xml` to the existing
`eduni/misc/001..009.xml`; the IDs are `hst-bh-001..006` and `hst-lhs-007..009`.
Both original and effective paths plus the correction label remain in the
result. No other path fallback is allowed.
