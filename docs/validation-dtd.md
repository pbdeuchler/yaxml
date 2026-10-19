# DTD validation

`yaxml::valid` reproduces libxml2 2.15.4's `valid.c`. A parsed DOM keeps the
declarations of its internal subset and of an external subset loaded through
a `Resolver` (`Document::dtd`, `OwnedDocument::dtd`).

| Call | libxml2 equivalent |
| --- | --- |
| `Document::validate`, `OwnedDocument::validate` | `XML_PARSE_DTDVALID` / `xmllint --valid`, with the checks libxml2 makes while parsing |
| `valid::validate(&tree, &dtd)` | `xmlValidateDtd` / `xmllint --dtdvalid` (no DOCTYPE-name or declaration checks) |
| `Dtd::parse(bytes)`, `Dtd::load(system, limits, resolver)` | `xmlParseDTD` |

Each call returns `Vec<Diagnostic<Id>>`. A diagnostic has a `Code` whose
discriminant is libxml2's `xmlParserErrors` value (`XML_DTD_*`, plus
`XML_ERR_UNDECLARED_ENTITY` 26 and `XML_ERR_ENTITY_BOUNDARY` 90), the element
concerned, the start of that element's source range, and a libxml2-style
message. `Code::is_warning` marks libxml2's validity warnings and
`valid::is_valid` ignores them. Well-formedness errors stay fatal `Error`s;
validity never fails a parse.

```rust
let xml = "<!DOCTYPE r [<!ELEMENT r (a+)><!ELEMENT a EMPTY>]><r><a/>text</r>";
let document = yaxml::Document::parse(xml)?;
let [diagnostic] = document.validate().try_into().unwrap();
assert_eq!(diagnostic.code, yaxml::valid::Code::ContentModel);
assert_eq!(diagnostic.node, Some(document.root_element().id()));
# Ok::<(), yaxml::Error>(())
```

## What is checked

- Element declarations: EMPTY, ANY, `(#PCDATA)`, mixed content (matched by
  local name, as libxml2), and children models. A children model compiles to
  its Glushkov automaton, so matching costs one lookup per child. A
  non-deterministic model (XML 1.0 Appendix E) is reported once per element
  type, like libxml2, and its content is not matched. Twelve models were
  checked against xmllint for agreement on determinism.
- Attributes: declared, lexically valid per type (libxml2's rules, e.g. NMTOKENS
  may lead with blanks), enumerations and notations, `#FIXED`, `#REQUIRED` with
  libxml2's prefix warnings, ID uniqueness, IDREF/IDREFS resolution, ENTITY and
  ENTITIES naming unparsed entities, namespace declarations.
- Declarations: redefined elements and notations, repeated mixed names and
  enumeration tokens, invalid or out-of-enumeration defaults, ID defaults and
  multiple IDs per element, NOTATION on EMPTY elements, undeclared notations of
  unparsed entities, repeated attributes (warning).
- Parse-time constraints: undeclared entities that unread declarations could
  have declared, PE nesting in declarations, groups and conditional sections,
  standalone defaults and normalizations from external declarations, and
  whitespace in externally declared element content. Parse-time diagnostics
  carry an input offset outside entity replacement text.
- The root element name, as `xmlValidateRoot`.

A children model's follow sets can be quadratic in its names. Construction
spends the DTD work budget and at most a quarter of the remaining DTD storage
budget (`Limits::max_owned_bytes`), and retained transitions are charged as
storage, so a series of 1300-name `(e0?, e1?, ...)` chains fails with
`LimitExceeded(OwnedBytes)` instead of allocating without bound. There is no
subset construction, so nothing is exponential.

## Results

W3C XML Test Suite 20130923 (`valid` and `invalid` cases), `OwnedDocument`
with a suite-rooted `FileResolver`, against `xmllint --valid --noout`:

```sh
cargo run --release --example xml_validity -- --manifest xmlconf-catalog.tsv \
  --suite-root xmlconf --expected-cases 2585 --xmllint ../yaxml-oracle/build/xmllint
```

| Cases | yaxml correct | libxml2 correct | Not parsed by yaxml |
| --- | ---: | ---: | ---: |
| 242 `invalid` reported invalid | 234 | 235 | 2 |
| 812 `valid` reported valid | 775 | 785 | 33 |

Wherever both parse, the verdicts agree except `rmt-e2e-15a` (below). Of the
33 `valid` cases yaxml does not parse, xmllint rejects 23 too; the other ten are
existing parser gaps (XML 1.1 text declarations), not validation. Besides
`rmt-e2e-15a`, yaxml accepts five `invalid` cases that libxml2 accepts too
(`rmt-e2e-15g`/`15h`, `rmt-ns10-045`/`046`, `hst-bh-005`). Per-case diagnostic
counts match `xmllint --valid --noent --dtdattr` on all but three cases,
explained below (`valid-sa-097`, `valid-not-sa-026`, `inv-not-sa13`).

libxml2's own `test/valid` corpus: each of the 30 of its 35 `.xml` files that
libxml2 parses has the same number of validity errors as `result/valid/*.err`
(`YAXML_LIBXML2_SOURCE=... cargo test --test valid -- --ignored`). Every row of
the constraint table in `tests/valid.rs` has the same number of validity
errors and warnings as xmllint (`YAXML_XMLLINT=...`).

## Fuzzing

The `valid` cargo-fuzz target validates input (raw, wrapped as an internal
subset, or an external DTD and document separated by NUL) over both DOMs and
requires identical diagnostics, deterministic results, node offsets that match
node ranges and a consistent `is_valid`. Seeds include deep nested repetition,
a 300-name optional chain and a 200-way repeated choice.

On `nightly-2026-06-06` two sessions ran 361 s (4,715,511 executions) and,
after bounding model construction by storage, 331 s (3,539,857 executions,
11,261 edges, 5,376-input corpus, peak RSS 697 MB) with no crash, leak or
timeout:

```sh
cargo +nightly-2026-06-06 fuzz run valid fuzz/corpus/valid fuzz/corpus -- -max_total_time=330
```

## Differences from libxml2

- Entities are always substituted, so an empty entity reference in an EMPTY
  element is invisible (`rmt-e2e-15a`) and IDs inside entity replacement
  text are registered (libxml2 with `NOENT` skips them).
- Defaulted attributes are always present and validated, as
  `xmllint --valid --dtdattr`.
- Declarations in an external parameter entity referenced from the internal
  subset count as external, as XML 1.0 defines external markup; libxml2 files
  them under the internal subset. This changes two repeated-attribute warnings
  (`valid-sa-097`, `valid-not-sa-026`) and, in such DTDs, notation redefinition
  and standalone whitespace checks.
- In a standalone document, libxml2 does not find externally declared
  unparsed entities for `ENTITIES` (but does for `ENTITY`); yaxml finds both
  (`inv-not-sa13` has one diagnostic instead of three).
- Standalone normalization is judged on the literal's whitespace; references
  are not expanded first.
- Declaration diagnostics have no node or offset, and messages do not render
  content models.
- The streaming readers do not validate; parse them into a DOM.
- A failed root-name check does not stop validation (as `--valid`, unlike
  `xmlValidateDocument`), and a missing external subset is not loaded at
  validation time: pass a resolver when parsing.
