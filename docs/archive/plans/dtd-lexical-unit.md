# Bounded DTD lexical-unit prototype

Base: `f2bd0438804660f8b9004e59743011f45e85d50d`.

This prototype validates one complete decoded XML 1.0 DTD unit at a time. It
does not make DOCTYPE acceptable to `Reader`, create public DTD events, expand
entities, apply attribute defaults, or perform external I/O. The module is
wired only into the crate's unit-test build until the semantic engine and
source-frame work are ready.

## Representation

`DtdUnit` owns or borrows the exact unit text through one `Cow<str>`. A single
fallibly grown `Vec<DtdToken>` stores validated UTF-8 ranges and flat structural
markers. The unit kind is a compact family tag; each validated family has fixed
root, identifier, and terminal-token positions. Getters slice ranges already
checked on construction; they do not rescan the raw declaration or allocate
field strings.

The parser accepts these complete unit forms:

- a DOCTYPE header ending in `[` or a no-subset header ending in `>`;
- ELEMENT declarations with EMPTY, ANY, mixed, or iterative children models;
- ATTLIST declarations with all XML 1.0 attribute types and default forms;
- general or parameter ENTITY declarations, including internal, external, and
  general unparsed NDATA forms;
- NOTATION declarations with SYSTEM, PUBLIC plus SYSTEM, or PUBLIC-only IDs;
- comments and processing instructions used as internal-subset items.

Entity values and attribute defaults retain their complete quoted lexical
spelling. Reference tokens distinguish named, decimal, hexadecimal, and
parameter references. Declaration views expose lexical facts only. They do not
claim processed replacement text, normalized default values, namespace
resolution, declaration binding, or validation results.

Element models remain flat. Parentheses, connectors, names, occurrence
operators, and `#PCDATA` are stored in source order. Construction uses an
explicit bounded group stack, so deeply nested models cannot recurse during
parsing or destruction.

## Bounds and failures

`DtdLimits` independently bounds unit bytes, token count, model depth, and
ATTLIST definitions. The byte ceiling is checked before any parser allocation.
Every token or stack growth checks its logical ceiling first, then uses fallible
reservation. Model depth is checked before growing the stack. Arithmetic uses
checked operations.

`DtdError` records a private error kind and an exact byte offset relative to the
unit's UTF-8 text. Limit errors identify the exhausted limit. Syntax errors
point at the first byte that cannot continue the selected production, including
EOF at `raw.len()`.

The parser reuses the crate's XML `NameStartChar`, `NameChar`, and XML character
predicates. Names in DTD grammar allow colons. Nmtokens require one or more
`NameChar` values. Character references must resolve to an XML character.
System and public literals, entity values, and attribute values keep their
different lexical grammars.

## Grammar and context boundary

The unit constructor enforces XML 1.0 productions and context-free
well-formedness available inside the complete unit. It rejects malformed
delimiters, references, literals, comments, PI targets, connector mixtures,
illegal parameter-entity declaration forms, and NDATA on parameter entities.

The unit constructor does not reject validity-only conditions. Duplicate
element declarations, duplicate mixed-model names, duplicate ATTLIST
definitions, nondeterministic children models, default membership/type
violations, undeclared notations, and duplicate notation/entity declarations
belong to later binding or validation policy. Parameter-entity placement and
expansion across unit boundaries also belong to the future source-frame engine.
An EntityValue may therefore contain a lexically valid parameter reference;
the engine will decide whether its occurrence context permits processing it.

## Test catalog

Table-driven tests cover every accepted unit family, owned and borrowed raw
storage, Unicode Names and Nmtokens, both quote styles, all reference spellings,
flat-model order, and exact typed-view values. Rejection tables cover every
grammar branch, malformed UTF-8-level characters, exact offsets, incomplete
units, trailing input, and limit boundaries.

Bounded tables vary whitespace, Unicode names, quote choice, and reference
values. Deterministic property loops vary model nesting and every UTF-8-safe
truncation of a representative declaration. Their oracles check typed lexical
facts, borrowed ranges, stable raw text, and parse-result invariants rather than
reimplementing the parser. Inputs and loop counts stay fixed and small.

## Activation gates

1. Complete the private lexical-unit grammar and independent catalog; run
   focused tests, strict Clippy, formatting, and deliberate grammar/limit fault
   checks.
2. Add the incremental DTD scanner and iterative source frames. It must extract
   bounded complete units without buffering the whole subset and enforce
   internal/external parameter-entity context rules.
3. Add first-binding GE, PE, and attribute state; replacement construction;
   defaults and declared-type normalization; cycle/work/output limits; and
   occurrence provenance. No external identifier triggers ambient I/O.
4. Wire the coherent semantics through Reader, push, buffered pull, and both
   DOM builders before exposing Begin/Item/End events, typed public views, DOM
   storage, or Writer support.
5. Remove the test-only module wiring and run source-complete fuzzing, XMLTS
   deltas, serialization/conformance checks, layout measurements, performance,
   and independent holistic review before merging DTD acceptance.

The prototype is evidence about the unit representation and grammar. Passing
its tests is not evidence that yaxml accepts or processes DTD-bearing documents.
