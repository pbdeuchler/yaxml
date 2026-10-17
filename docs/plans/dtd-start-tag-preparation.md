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

`ParserState::prepare_explicit_tag` scans the raw tag, validates element and
attribute QNames, rejects repeated lexical attribute names, enforces the
explicit attribute ceiling, and applies the existing DTD-free normalization.
`ParserState::publish_prepared_tag` is the only namespace-resolution and scope
publication path. The existing DTD-free `start_element` calls these two methods,
so ordinary parsing remains compiled and behavior-tested.

The private `dtd_tag` module adds one concrete preparation function taking
`&mut DtdCatalog`, `&mut Budget`, the raw tag and its physical offset. It uses
the same lexical collector, replaces explicit values with the catalog's
declared-type normalization, and appends applicable defaults. It returns a
`PreparedTag` or a typed `DtdTagError` preserving parser errors, catalog errors,
and unavailable-default dependencies. `DtdController` will eventually own the
call and retain that first error through its existing sticky failure state;
this checkpoint tests the preparation function directly and does not create a
second persistent failure field.

No callback or trait abstracts normalization. The DTD-free and DTD-aware
preparers are the two concrete policies. Both feed the same `PreparedTag` and
publication code.

## Required order and errors

Preparation and publication perform these steps:

1. Validate the tag delimiters, empty marker and element QName.
2. Collect every explicit lexical attribute in source order. Validate each
   QName, reject a repeated qualified name, and stop at the explicit attribute
   ceiling before normalizing values.
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

The deliberate precedence change from the old DTD-free implementation is that
an exact repeated lexical QName is rejected before normalizing the repeated
value or publishing namespaces. All other established DTD-free lexical,
namespace, expanded-duplicate and limit errors retain their existing kinds and
physical offsets. A default-name error uses its declaration ordinal rather than
inventing a physical input offset.

## Ownership, limits and transaction boundary

Explicit names and unmodified values continue to borrow the current input where
possible. Catalog-normalized explicit values move their returned `String` into
the prepared tag after one shared-budget charge. Defaults borrow catalog data
only during preparation; their name and normalized value are copied once into
owned event fields after charging the shared budget. Namespace scope retention
may require a separate owned copy because it outlives a lending event; that copy
is charged separately and is not a duplicate event copy.

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
