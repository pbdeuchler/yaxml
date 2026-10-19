# DTD representation decision

Recommendation: a Begin / Item / End logical event stream, collected by DOM
builders into one DOCTYPE node containing a header and an ordered structured
item catalog. Keep parser binding indexes separate from that catalog. Introduce
these shapes together with actual internal-DTD grammar and semantics, not as
unused public variants or an empty-DOCTYPE success milestone.

The serialization contract needs a decision now: recommend normalized DTD
serialization after PE expansion, with included internal parameter entities
flattened into ordered declaration items. Preserving parameter-reference spellings is a later
reference-preserving mode. A stream must not emit both `%p;` and its replacement
declarations as independently serializable items: replay would duplicate them.

## Comparison

| Shape | Streaming and ownership | Ordering/serialization |
| --- | --- | --- |
| One Doctype event with complete raw subset | Reader can borrow a source slice, but Push must retain the complete subset before delivery, alongside bindings. A bounded structured Vec avoids concatenation but still retains all declarations before delivery. Returning a parser-owned catalog through Reader's input lifetime is invalid; moving it loses parser binding ownership unless copied/shared. | Easy DOM shape, poor incremental protocol. A raw subset alone requires reparsing for useful declaration access. |
| Begin / Item / End logical events | One bounded declaration unit plus semantic tables. Reader borrows direct input; Push lends current-token fields. Replacement-origin fields require fallible owned promotion. | Preserves logical order and enables bounded output one declaration at a time. DOM collects at one prolog position. Requires explicit logical-versus-physical PE policy. |
| Header event plus parser catalog accessor | Semantic bindings are already retained, but a borrow of parser state cannot survive the next read. A durable catalog snapshot needs cloning, sharing or transfer. | Header alone cannot drive a complete streaming writer; closure and ordered declaration notifications recreate Begin/Item/End. Useful as DOM storage, not a replacement event protocol. |

A maximum aggregate DTD storage limit makes a full-subset event finite, but does
not make it equally incremental: delivery, cancellation and temporary storage
still depend on the entire subset. It is not the preferred streaming baseline.

## Public event shape

Conceptual additions, not source changes:

```rust
Event::DoctypeBegin(DoctypeHeader<'a>)
Event::DtdItem(DtdItem<'a>)
Event::DoctypeEnd

// Compact validated unit handles; getters produce typed borrowed views.
struct DoctypeHeader<'a>(DtdUnit<'a>);
struct DtdDeclaration<'a>(DtdUnit<'a>);

struct DtdUnit<'a> {
    raw: Cow<'a, str>,
    tokens: Vec<DtdToken>,
    kind: DtdUnitKind,
}

enum DtdItem<'a> {
    Declaration(DtdDeclaration<'a>),
    Comment(Cow<'a, str>),
    ProcessingInstruction { target: Cow<'a, str>, data: Cow<'a, str> },
}

// Views borrow the declaration item, even when its raw text is owned.
enum DtdDeclarationView<'item> {
    Element(ElementView<'item>),
    AttributeList(AttributeListView<'item>),
    Entity(EntityView<'item>),
    Notation(NotationView<'item>),
}
```

Keep comments/PIs inside DTD items, rather than making callers infer their
container from ordinary content events. DTD items never enter the element
namespace stack or appear as ordinary children of the document root.

Minimum declaration information exposed by these typed views (not nested owned
Cow payloads stored inline in Event):

- Element: lexical element name and a validated flat model token sequence.
  EMPTY, ANY, mixed content and children models are distinguishable. Use a flat
  Vec and iterative group checking; no recursively dropped public Box tree.
- AttributeList: lexical element name and ordered iteration over attribute definitions.
  Each has lexical attribute name, declared type (including enumeration or
  notation names) and Required / Implied / Default / Fixed form. One ATTLIST
  remains one declaration unit; limits bound both unit bytes and definition
  count. No need for arbitrary attribute-list event fragments.
- Entity: separate general/parameter identity, lexical name and internal value
  or external identifier. General external entities can additionally carry
  NDATA notation; parameter entities cannot. The representation/validator must
  preserve that distinction.
- Notation: lexical name and System / Public identifiers, including the legal
  PUBLIC-only form. Do not reuse an external-ID type requiring a system literal
  without a notation-specific Public-only case.

Entity values and attribute default literals must retain their lexical reference
spelling, distinctly from processed replacement/default values. A practical
minimal view is `quoted_literal() -> &str`, selecting the XML quote delimiters
and unexpanded reference spelling from the validated unit. Source decoding and
physical EOL processing remain distinct from replacement construction. Validate
the literal in its specific EntityValue or AttValue grammar. The serializer may retain this quoted
literal verbatim while normalizing spacing around it. It must not route it
through ordinary text escaping or replace it with a normalized attribute value.

This avoids constructing the wrong declaration for numeric CR, ampersands or
markup created at later entity use. Semantic binding storage separately owns or
borrows processed replacement text and normalization inputs. No whole raw
DOCTYPE copy is needed just to preserve these phase distinctions.

Expose borrowed lexical-name strings through typed views; resolved Name namespace
metadata is not appropriate before instance namespace processing. Public event construction
remains checked by Writer, like current mutable event fields; no unchecked Name
constructor or trusted public DTD escape hatch is needed.

## Observable event/provenance order

For an internal subset, Begin covers the validated header through `[`. Each
item covers its own declaration/comment/PI. End covers `] S? >`. Whitespace
between units need not be retained. DOCTYPE without a subset still emits Begin
and End: its Begin excludes the final `>` and End covers that delimiter. Thus
there is no fake zero span, duplicate whole-DOCTYPE span or backward join.
A withheld delimiter is constant lookahead; emitting it can require draining a
pending logical event without consuming another physical byte.

Only End completes the DOCTYPE. Delivering Begin or earlier items before a
later syntax error is valid-prefix streaming, not acceptance of the document.
EOF inside DTD remains terminal failure. Before the first root event, all read
internal declarations and their effects must have been handled.

Included internal PE replacement declarations emit the same logical item kinds,
with replacement occurrence provenance. The public sequence contains no physical
`%p;` item in this logical mode. For example, an internal PE containing an ATTLIST
produces its PE declaration followed by that ATTLIST at the reference's logical
position. Serialization writes those declarations and omits the expansion-site
reference. It does not write both forms.

A future mode that intentionally skips a PE needs an explicit skipped-reference
notification, declaration-processing barrier and serialization contract. The
current representation must not claim completeness for declarations never read.
External-resource support also needs base-URI-aware serialization decisions;
flattening external declarations with relative identifiers is not automatically
safe. Neither issue justifies ambient I/O or silent skipping in the initial
internal-DTD capability.

## DOM shape

Add one Doctype kind to borrowed and owned node categories at the actual document
child position. For `comment, doctype, PI, root`, serialization/traversal retains
that order. Do not store DOCTYPE solely beside XML Declaration: Declaration is
necessarily first, while DOCTYPE has meaningful relative position among Misc.

The node references one document-owned header plus ordered located DTD items.
Do not put a large inline Doctype payload into NodeKind: that can enlarge every
ordinary tree record. A private zero-or-one Vec of Doctype storage permits
fallible allocation only when a DTD actually exists; a unit Doctype node resolves
to that sole record through Node::doctype(). This is one special document slot,
not a general source/type arena. Owned storage uses existing pool ranges and
stored item/model indexes, exposing borrowed views without reparsing.

The DOCTYPE is one main-tree node. DTD declarations are iterated through its
catalog rather than pretending to be ordinary XML child elements. Each item has
its own origin; the enclosing node has the complete direct input DOCTYPE span,
even when some logical declarations came from PE replacements.

The catalog retains order, duplicate declarations and comments/PIs for access
and serialization. Effective binding indexes retain first bindings only. A
HashMap iteration order or first-binding-only table cannot replace the catalog.

## First bindings and lifetime costs

Private GE and PE maps are separate and point to stable declaration IDs. An
entity frame uses declaration ID for its replacement text and a fresh occurrence
identity for each invocation's origin. Do not equate those identities.

Attribute bindings use lexical (element, attribute) matching, with ordered first
bindings per element for default application. An ATTLIST can introduce a mix of
new and already-bound attributes, so one declaration-wide active flag is wrong.
Later duplicate declarations still undergo required grammar checking and can be
reported/stored; they do not replace effective earlier bindings.

Reader can borrow each direct declaration unit from its complete input. A unit
created from parser-owned replacement storage cannot borrow that storage for
Reader's input lifetime; promote the bounded raw unit into one budgeted Cow.
Its validated relative token ranges remain usable without per-field copying.
Public events must not borrow the semantic catalog. No generic all-Event-to-static conversion
is needed, and XML Declaration remains unchanged.

Push/Buffered can lend fields from the current bounded unit. Before releasing
it, copy only data needed by retained effective bindings. Duplicate declarations
need not retain semantic payload copies in streaming mode. Callback item metadata
(flat field/model token ranges) is unit-local and bounded. DOM consumers separately
retain all ordered items; their additional full-DTD storage is expected and
subject to aggregate storage limits. Owned DOM copies needed payload into its
pool. Retaining both a full raw subset and the structured items is unnecessary.

Keep an explicit budget split between bounded current declaration scratch,
cumulative grammar/reference work, effective declaration storage, output events,
expanded text and DOM-retained catalog storage. Empty entities and duplicate
large declarations can consume work without contributing effective bindings.

## Normative checks affecting this decision

The prolog permits Misc around a single DOCTYPE before the root; internal subsets
contain declarations, comments, PIs and declaration separators. Internal PE use
is constrained to between declarations. [XML 1.0 section 2.8](https://www.w3.org/TR/2008/REC-xml-20081126/#sec-prolog-dtd)

Attribute and entity duplicates use first-binding rules. [Sections 3.3 and 4.2](https://www.w3.org/TR/2008/REC-xml-20081126/#sec-attlistdecl), [entity declarations](https://www.w3.org/TR/2008/REC-xml-20081126/#sec-entity-decl)

Literal entity spelling and replacement text differ; character references are
processed during replacement construction while general references remain.
[Section 4.5](https://www.w3.org/TR/2008/REC-xml-20081126/#sec-entexpand)

Nonvalidating processing still applies read internal declarations. An unread PE
changes later entity/attribute processing, with a standalone-dependent exception.
[Section 5.1](https://www.w3.org/TR/2008/REC-xml-20081126/#proc-types)

Notation identifiers can be PUBLIC-only. [Section 4.7](https://www.w3.org/TR/2008/REC-xml-20081126/#Notations)

## Decision boundary

Approve logical expanded declaration events plus an ordered DOM catalog only if
normalized DTD serialization after PE expansion is the intended initial
contract. If reference-preserving serialization is required immediately, choose a physical
DTD syntax ledger and a separate semantic declaration channel instead; that is
meaningfully more machinery, not an implementation detail to hide.

The acceptance milestone remains integrated internal DTD grammar, internal
entity behavior, defaults and declared-type normalization across all five APIs,
with occurrence provenance before generated events. The public representation
must be implemented as part of that capability, not used to redefine success
as header-only or empty-only DOCTYPE support.

Read-only design based on source-origin production tree28ac931. No source edits,
builds, timing, new acceptance paths or external resource operations performed.

## Event layout and allocation follow-up

The initial fully inline Cow sketch was too expensive: keeping NodeKind compact
does not protect Event. Recommendation now: compact validated declaration AND
header units, exposing typed borrowed views. The sketch above reflects this.

All following sizes are rough models on 64-bit targets, not measured layouts.
Current measured Event is 88 bytes, with an 80-byte Element payload. Cow<str>
and Vec each occupy 24 bytes in the current compiler.

| Representation | Modeled Event effect | DTD-specific costs |
| --- | --- | --- |
| Nested inline Entity / ExternalId / Declaration / Item payloads | About 144 bytes in a straightforward tag/alignment model; compiler niches may reduce this | No payload wrapper allocation, but a larger enum affects ordinary parsing paths too |
| Raw Cow + flat validated token Vec + compact unit kind | Unit about 56 bytes; DtdItem about 64; Event can plausibly remain 88 if header uses the same compact strategy | One metadata allocation per unit, plus raw text only when ownership promotion is needed |
| Typed payload behind a private singleton Vec | Handle 24 bytes; Event can plausibly remain 88 | Extra per-unit allocation/indirection, plus the typed payload's own lists/strings |
| Typed payload behind Box | Handle one word; Event can plausibly remain 88 | Box::new does not provide this crate's allocation-error contract; Box::try_new is not stable in Rust1.95 |

One illustrative inline entity path is ExternalId about56, plus optional NDATA
name24 and definition tag8 => EntityDef88; entity name24 and entity-kind/tag8 =>
EntityDeclaration120; enclosing declaration/item/event tags can reach144. This
is only a planning model. Rust layout/niche optimization is not a public ABI.

The inline header was independently problematic: root-name Cow24, optional
ExternalId roughly56, subset flag/alignment about8 => header88, making Event
roughly96 even after compacting declarations. Compact the header too. A fixed
public collection of Option<Range> fields is not automatically smaller; header
field ranges can live in its bounded metadata Vec instead.

Do not duplicate SourceOrigin inside the unit: it already accompanies events.
No packed offsets, u32-only length ceiling or custom niche/sentinel encoding is
needed to hit the 56-byte unit model. Token records can initially use ordinary
usize ranges; their heap cost, roughly24–32 bytes per token depending on links,
must be charged and bounded. Inspect actual layout and full ordinary workloads
before committing this representation to the public API.

### What the compact unit owns

The raw text is exactly one validated lexical declaration or DOCTYPE header,
not a whole subset. Relative ranges identify names, external identifiers,
quoted literal spellings and model terms. Unit construction checks the complete
unit grammar and range/character boundaries once, and keeps fields private.
Context-dependent declaration rules remain the parser/Writer's responsibility.
Typed accessors borrow the item: they do not promise the input lifetime when the
unit owns its raw text. This follows the Name ownership precedent.

EntityView and similar views can contain a unit reference and token index.
Getters slice already-validated ranges; attribute/model iteration walks the flat
index. Store enough boundaries to avoid reparsing or rescanning the entire unit
on every getter. The public typed view can have expressive enums without putting
those enums into every stored Event. Its lifetime ends with the item/view owner.

A public fallible unit constructor can validate lexical syntax with offsets
relative to byte zero. Parser processing additionally validates declaration
context and applies first bindings before delivery. Writer checks context in
its own state, rather than trusting arbitrary user-constructed declaration text.
No public unchecked constructor or mutable raw-text field is needed.

### Semantic state and duplication

The public/DOM catalog owns the ordered lexical units and their validated index
metadata, including duplicates. Parser first-binding state owns stable binding
IDs, name indexes, processed entity replacement text and attribute rules/default
normalization inputs. It is not a second copy of every declaration tree.

Streaming retains only effective binding payloads after a unit is released.
Comments, ignored duplicate declarations and their token metadata can disappear
after callbacks. Unchanged effective strings may borrow complete input in
Reader; temporary streaming text needs bounded copies. A generated Reader unit
needs one raw-buffer promotion, not a new allocation for each field view.

DOM parsing intentionally retains all ordered units. Binding-specific processed
text may still coexist with raw quoted spelling when expansion/normalization
changes it; that is necessary phase data, not redundant lexical storage. After
parsing, discard parser binding indexes unless a concrete DOM API requires them.
If retained for later APIs, index the DOM catalog and keep only computed data;
do not duplicate another raw subset or token tree. Parser-private IDs must be
remapped or otherwise given document-owned meaning before the parser disappears.

Owned DOM can retain raw unit text in its existing string pool and token ranges
relative to the unit/pool entry. It must not both pool the raw unit and copy all
of its unchanged lexical fields into another owned typed tree. Metadata remains
flat and fallibly allocated. Extra allocations caused by a singleton Vec payload
are justified only if that alternative proves simpler after measurement.

### Terminology and verified allocation constraint

Call this normalized DTD serialization after PE expansion, or simply DTD
serialization after PE expansion. Define any chosen whitespace/quote policy
separately. It is not W3C Canonical XML: C14N removes the XML declaration and
DOCTYPE. [C14N1.1 section1.1](https://www.w3.org/TR/xml-c14n11/#Terminology)

Rust1.95's pinned Box source marks try_new allocator_api-unstable; replacing a
fallible Vec with Box::new would change allocation-failure behavior. Do not
assume newer allocator stabilization settles this API at the crate's current
MSRV. [Rust1.95 boxed.rs](https://raw.githubusercontent.com/rust-lang/rust/1.95.0/library/alloc/src/boxed.rs)

No builds, benchmarks, source edits or integration changes were made for this
follow-up. The numerical model must be replaced by compiler measurements when
implementation is authorized.
