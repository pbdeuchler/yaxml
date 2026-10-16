# XML conformance regression scope

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
