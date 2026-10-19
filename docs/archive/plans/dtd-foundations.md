# DTD implementation foundations

Base: `2609aeff4c5287b5fbbdadd15eb55b1cbd296aa0`.

DTD processing is the largest current conformance gap. Implement literal
declarations, entity replacement, defaults and normalization as one coherent
semantic engine shared by Reader, both DOMs, push and buffered pull. Keep the
larger feature/API inventory and security requirements in scope; these stages
are implementation order, not a reduced definition of libxml2 compatibility.

## Ownership foundation

Current `Name` stores three borrowed slices and PI targets are also borrowed.
They cannot represent names/targets produced by parser-owned entity input while
preserving `Reader::read_event`'s useful input lifetime. First make those values
owned-capable without changing parsing behavior or enabling DTD acceptance.

Use one `Cow<str>` QName, a private `Option<NonZeroUsize>` prefix length, and
optional namespace metadata. Derive prefix/local views from that one buffer;
do not store three independently owned strings. Make all fields private and
expose `qualified()`, `prefix()`, `local()` and `namespace()` getters. Their
references borrow the name view because its buffer may be owned.

Add `Name::new(qualified: impl Into<Cow<str>>, namespace: Option<Cow<str>>)`
returning `Result<Name, Error>`. It validates the lexical QName with error
offsets relative to byte zero, allocating nothing for borrowed input and moving
supplied owned buffers. Preserve namespace metadata as supplied: namespace
context consistency remains the parser/writer's responsibility. This permits
lexically valid names to be prepared before their namespace scope exists.

Keep a narrow crate-private constructor for already-validated QName/split
metadata, with constant-time debug assertions. Parser and stored-name view
callers must derive the split from validated data. Preserve constant-time
`OwnedNode::name()` and borrowed serializer views; do not validate or search the
whole QName on every view construction. Avoid a generic validated-storage
framework. A private `as_borrowed()` helper can preserve metadata directly.

Change PI targets in `Event` and borrowing `NodeKind` to `Cow<str>`. Keep direct
input borrowing and existing budgeted streaming retention. Update builders,
writer, serialization, examples, benchmarks, tests and fuzz harnesses together.
Declaration fields remain borrowed. Do not add an event-to-static conversion
that fails on declarations or pretends arbitrary declaration text is static.
Fallible ownership promotion for replacement content belongs with entity frames.

This intentionally changes the young Rust API from mutable name fields to
validated construction and getters. Migrate forged lexical-metadata tests to
constructor rejection; retain writer namespace-context mismatch tests. Do not
add an unchecked public or test-only bypass. Document migration examples and
the shortened getter borrowing lifetime. No new dependency or unsafe code.

Required evidence: independent constructor/getter/ownership tests, accepted and
rejected Unicode QNames with exact offsets, owned PI writer/DOM round trips,
all five parsing surfaces, existing error/quota/serialization tests, and fuzz
dispatch for the new input-bearing constructor. Measure actual size/alignment
of Name, Attribute, Element, Event and borrowing NodeKind before/after. Run the
frozen DTD-free paired performance matrix and investigate material regressions;
record fresh source-complete fuzz coverage and complete XMLTS results. Review
every changed line and deliberately break lexical validation, split metadata,
and owned PI handling before integration into local main.

## Remaining DTD architecture

After ownership, introduce real source provenance before generated markup.
Contiguous original-input ranges must stay distinct from replacement offsets
and reference origins. Both DOM builders need provenance-aware element closure;
the owned builder currently assumes parsed end events always have source ranges.
Physical stream position must count consumed document bytes, not expansion.
The [entity text-boundary decision](entity-text-boundaries.md) records pinned
DOM/Reader/SAX probes, bounded streaming fragmentation, parse-only DOM
coalescing and exact versus mixed provenance requirements.

Build an incremental DTD grammar with bounded declaration units rather than an
unbounded whole-DOCTYPE scratch buffer. Retain structured first-binding entity
and attribute declarations, separate GE/PE tables, and flat element models.
Complete grammar checking must precede general nonempty-subset acceptance.
DOCTYPE nodes/events and serialization need explicit prolog-order semantics.

Use iterative source frames keyed by declaration ID, with cursor, reference
origin, entry element depth and lexical state. Store a bounded replacement
string per entity, plus source mapping where needed. Never concatenate an
expanded whole document. Content expansion emits ordinary parser tokens;
attribute expansion uses a separate literal context. Add independent cumulative
limits for DTD storage/declarations/work, reference attempts, expansion bytes,
active depth and default attributes before activating replacement. Cycles and
empty-entity amplification need checks independent of output size.

Before resolving a start tag, parse explicit lexical attributes, apply read
declarations/defaults and declared-type normalization, collect explicit/default
namespace declarations, then resolve all names and reject expanded duplicates.
Keep declaration matching lexical and expose specified/defaulted provenance.
Validation of content models, IDs and other validity constraints is a later
mode; nonvalidating processing must not reject validity-only failures.

## Normative traps and retained probes

Character references in EntityValue construct ordinary replacement text, which
is reparsed at use; references recognized during content processing yield final
data at that level. Thus declaration `&#60;a/>` can create an element, while
direct content `&#60;a/>` remains text. Internal-subset PE references inside
markup declarations are forbidden. Apply defaults and read declaration effects;
after an unread PE, later entity/attribute declarations have a standalone-dependent
barrier. [XML 1.0 §§2.8, 4.4–4.5, 5.1](https://www.w3.org/TR/2008/REC-xml-20081126/)

Literal EOL normalization and character-reference expansion are separate phases.
Do not normalize internal replacement text's numeric CR again as physical input.
Defaults can introduce namespace bindings on the current tag, so apply them
before namespace resolution. [XML §§2.11, 3.3.3](https://www.w3.org/TR/2008/REC-xml-20081126/),
[Namespaces §6.1](https://www.w3.org/TR/REC-xml-names/#scoping)

The pinned 2.15.4 `xmllint` probes in
`/private/tmp/yaxml-dtd-reference-phases` confirm numeric-markup, direct-numeric,
ampersand no-rescan and forbidden internal-PE cases. They also reveal a policy
difference requiring explicit review: XPath text for numeric CR inside an
internal entity is LF, while direct numeric CR remains CR, both with and without
substitution. Retain this observation rather than assuming oracle/spec agreement.
The directory includes exact commands, exit codes, stdout bytes, diagnostics,
binary/static hashes and linked-library inventory. Earlier unread-PE probes are
in `/private/tmp/yaxml-dtd-normative-audit`.

External identifiers do not authorize I/O. Keep unsupported external dependencies
explicit until resolver and skipped-entity notification contracts exist. Later
work includes internal PEs, intentional skip behavior, explicit resolvers,
reference-preserving nodes/events, declaration access and DTD validation. Each
activation needs its own conformance delta, independent expansion model,
chunk/UTF-16 diagnostics, resource/cycle tests and fuzz evidence.
