# Private staged start-tag preparation

Base: `bc59b74d97946ca68ff1e00bbb6478afd70a0afa`, plus the reviewed
provenance test follow-up cherry-picked as `06d55cb`. This checkpoint remains
private and test-only for DTD-aware behavior. It does not accept a DOCTYPE in a
public parser, expand general entities in content, add public attribute fields,
or change Reader/Stream event protocols.

The current `ParserState::start_element` installs namespace bindings while it
is still scanning and normalizing explicit attributes. That order cannot apply
DTD defaults which themselves declare namespaces, and a later error can leave
part of the namespace scope published. Replace it with one common staged tag
representation consumed by the existing namespace implementation.

## Concrete representation and entry points

Keep the types in `parser.rs`; this is one parser operation rather than a new
framework:

```text
RawAttribute<'event> {
    qualified: &'event str,
    prefix_length: Option<NonZeroUsize>,
    literal: &'event str,
    name_offset: usize,
    value_offset: usize,
    value_end: usize,
}

RawTag<'event> {
    qualified: &'event str,
    prefix_length: Option<NonZeroUsize>,
    attributes: Vec<RawAttribute<'event>>,
    empty: bool,
    start: usize,
}

AttributeProvenance = Specified { name_offset, value_offset }
                    | Defaulted { declaration }

PreparedAttribute<'event> {
    name: Name<'event>,
    value: Cow<'event, str>,
    provenance: AttributeProvenance,
}

PreparedTag<'event> {
    name: Name<'event>,
    attributes: Vec<PreparedAttribute<'event>>,
    empty: bool,
    start: usize,
}
```

`ParserState::collect_raw_tag` scans the raw tag, validates element and
attribute QNames, rejects repeated lexical attribute names, enforces the
explicit attribute ceiling, and retains each raw literal spelling and its
exact byte range. It performs no value normalization. `RawTag` then has two
concrete consumers: the DTD-free policy applies the existing normalization,
while the DTD-aware policy calls `DtdCatalog::normalize_attribute_value`
exactly once. A prepared value is never normalized by one policy and replaced
by the other.

`ParserState::publish_prepared_tag` is the only namespace-resolution and scope
publication path. The existing DTD-free `start_element` calls these two methods,
so ordinary parsing remains compiled and behavior-tested.

The private `dtd_tag` module adds one concrete preparation function taking
`&mut DtdCatalog`, `&mut Budget`, a `RawTag`, and an explicit `LiteralSource`.
`Physical` literals receive physical end-of-line normalization; `Replacement`
literals have already crossed that boundary and must not be normalized as
physical input again. It applies the catalog's declared-type normalization and
appends applicable defaults. It returns a
`PreparedTag` or a typed `DtdTagError` preserving parser errors, catalog errors,
and unavailable-default dependencies. `DtdController` will eventually own the
call and retain that first error through its existing sticky failure state;
this checkpoint tests the preparation function directly and does not create a
second persistent failure field.

The private signatures are intentionally concrete:

```text
ParserState::collect_raw_tag(raw, local_start) -> RawTag
ParserState::prepare_physical_tag(RawTag) -> PreparedTag
prepare_dtd_tag(catalog, budget, RawTag, LiteralSource) -> PreparedTag
ParserState::publish_prepared_tag(PreparedTag, retain) -> Event
ParserState::element_depth() -> usize
```

`local_start` and every collected range are relative to the current token
source. The controller keeps the causal `FragmentOrigin` separately; it is not
stored redundantly in every prepared attribute.

No callback or trait abstracts normalization. The DTD-free and DTD-aware
preparers are the two concrete policies. Both feed the same `PreparedTag` and
publication code.

## Required order and errors

Preparation and publication perform these steps:

1. Validate the tag delimiters, empty marker and element QName.
2. Collect every explicit lexical attribute spelling and byte range in source
   order. Validate each QName, reject a repeated qualified name, and stop at
   the explicit attribute ceiling before normalizing values.
3. For DTD-aware preparation, normalize every explicit value against the first
   lexical `(element QName, attribute QName)` declaration. Preserve catalog
   failures and unavailable dependencies. Charge a successfully returned owned
   value to the shared parser budget before moving it into the prepared tag.
4. Iterate first-binding declarations for the lexical element name in declaration
   order. Skip required/implied entries and any lexical name supplied explicitly.
   For each applicable value default, validate its QName only now, copy its name
   and normalized value once into the prepared event storage under the shared
   budget, and append it with its declaration ordinal. An unused DTD Name may
   contain multiple colons and is never rejected merely because it was declared.
5. Check the final explicit-plus-default attribute ceiling before retaining an
   additional default. An explicit ceiling failure therefore precedes value
   normalization; a final ceiling failure identifies the first default that
   would exceed it.
6. Validate all explicit and defaulted namespace declarations against the
   completed attribute list. Precompute URI identities, prior-prefix links,
   namespace map additions and every retained scope string. Reserve all affected
   vectors/maps and the nonempty-element frame before semantic publication.
7. Resolve the element and all ordinary attributes against that planned scope,
   then reject duplicate expanded ordinary names.
8. Commit the prepared namespace bindings and element frame with only infallible
   operations. Empty tags unwind the just-committed scope after constructing
   the event; nonempty tags retain it until the matching end tag.

DTD matching is case-sensitive and lexical, before namespace resolution. A
defaulted `xmlns` or `xmlns:p` can bind the element or an ordinary attribute on
the same tag. Explicit attributes keep source order and defaults append in
effective first-binding declaration order, including namespace declarations.
Fixed-value mismatch, required attributes, enumeration membership, ID rules and
other validity constraints remain outside this nonvalidating slice.

Staging deliberately changes more than repeated-name precedence. The collector
finishes lexical validation before either normalization or namespace checks,
and a preparer normalizes all explicit values before publication validates the
completed namespace list. For example, a later malformed attribute can now
precede a bad earlier value, while a bad value can precede an otherwise earlier
namespace error such as an empty prefixed binding. Author tests pin before/after
controls for these cases, and activation compares the chosen results with the
pinned libxml2 oracle. Error kinds and physical offsets remain exact within the
chosen staged order. A default-name error uses its declaration ordinal rather
than inventing a physical input offset.

## Ownership, limits and transaction boundary

Physical-input names and unmodified values continue to borrow the current token
where possible. Replacement-token element and explicit-attribute names are
promoted to owned `Name` buffers under the shared budget before the controller
can release or mutate its scratch/catalog state. Catalog-normalized explicit
values move their returned `String` into the prepared tag after one shared-budget
charge. Defaults borrow catalog data only during preparation; their name and
normalized value are copied once into owned event fields after charging the
shared budget. No catalog borrow survives a normalization call. Namespace scope
and a nonempty element frame may require separate owned copies because they
outlive a lending event; those copies are charged separately and do not forge an
`'event` lifetime. `PreparedTag` stores owned-capable `Name`/`Cow` values and is
consumed by publication, so generated input never requires a blanket
`Event<'_>` to `Event<'static>` conversion.

Parser and catalog errors carry token-local UTF-8 byte offsets. The controller
retains the separate causal `FragmentOrigin` for the current physical reference
and maps an error only at its owning boundary; a replacement-local offset is
never passed through the physical UTF-16 offset mapper. `ParserState` exposes a
crate-private `element_depth()` accessor. The entity-frame pre-end guard must run
against that depth before `end_element` mutates stack or namespace state; this
start-tag checkpoint does not duplicate the guard inside namespace publication.

Attribute `Vec` and lexical/expanded duplicate sets reserve fallibly. Size sums,
the next namespace identity and binding counters use checked arithmetic. Failed
preparation may consume catalog work and shared budget attempts, but it publishes
no namespace binding, element frame, attribute or event. Catalog failures are
already sticky; the composed controller remains the single owner of cross-stage
sticky failure. No allocation is hidden behind indexing or `collect`.

## Verification and remaining gates

Author tests live at `tests/internal/dtd_tag_author.rs`. They cover explicit
DTD-free parity, declared CDATA/tokenized normalization, ordered defaults,
defaulted namespace bindings, explicit suppression, lexical and expanded
duplicates, invalid applied versus unused multi-colon DTD names, every namespace
constraint, explicit/final attribute ceilings, unavailable defaults, catalog
errors, shared-budget exact boundaries, reservation failures where the existing
seam permits them, publication atomicity and specified/defaulted provenance.

Fault checks must break at least: defaults-before-namespace order, lexical
duplicate detection, final attribute counting, declared-type normalization and
publication atomicity. Public DTD acceptance, GE content frames, SourceOrigin
delivery, Reader/Stream parity, Writer/DOM DTD retention, XMLTS, fuzz coverage
and performance remain later activation gates.
