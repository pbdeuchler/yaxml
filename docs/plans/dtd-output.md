# Phase-aware private DTD output

Base: `da41836`. This is a private field encoder for validated DTD units,
not public Writer activation or a second DTD parser. The existing logical
Begin/Item/End protocol and complete activation gates remain required.

A physical declaration can be copied as validated raw text: reparsing performs
the same physical EOL phase. A declaration generated in a PE is already
replacement text. Copying a literal CR into physical output would renormalize
it, changing later declarations and instance values.

Use the validated flat literal-token ranges to emit borrowed physical fragments:

- Physical units retain their complete raw spelling.
- In Replacement EntityValue, emit literal CR as `&#13;` so construction restores
  CR after physical EOL handling. Preserve reference spelling verbatim.
- In Replacement AttValue, emit literal CR as U+0020: ordinary literal XML
  whitespace becomes space at attribute normalization. Numeric CR references
  remain references, retaining their distinct semantics. CRLF must still yield
  two spaces, unlike physical CRLF's single normalized LF.
- Reject Replacement CR inside system/public identifier literals with a typed
  unit-local unrepresentable-identifier error. Character references are not
  escapes in those grammars; silently changing the identifier is unacceptable.
- Comments/PIs follow Writer's existing documented raw-CR normalization policy.
  Other declaration whitespace is semantically interchangeable after EOL
  handling. No namespace or general text escaping applies to a DTD literal.

Expose one private fused fragment iterator on DtdUnit, with an explicit literal
phase argument. It returns borrowed raw slices, static replacement spelling, or
one typed error. Iterate flat literal metadata and CR positions monotonically;
do not search every token for every CR, allocate a rewritten intermediate unit,
or reparse the declaration. The eventual Writer appends these fragments to its
existing bounded current-event buffer and validates the encoded result before
any sink bytes. Its Begin/End delimiter handling remains separate.

The phase remains retained by the logical item/controller and eventual DOM
storage; it cannot be inferred from raw text or an optional original-input range.
The prototype iterator takes the phase explicitly without adding a second
persistent source flag. Public construction and retention will freeze with
actual Writer integration.

Tests compare literal bytes for every declaration family, direct vs replacement
phase, multiple ATTLIST definitions, numeric vs literal CR, CRLF, Unicode,
quoted references and unrepresentable identifiers. Compose output with a fresh
lexical unit and catalog to compare entity replacement/default semantics, using
literal expected values rather than deriving an oracle from encoded output.
Test iterator fusion and exact error offsets. The iterator allocates nothing;
actual Writer output/quota/I/O tests remain activation requirements.
