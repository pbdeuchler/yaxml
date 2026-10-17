# Blind conformance plan for a private stable DTD reader

Audited accepted commit `992c177e8fd93650087e713f3ff99d13404c22fa` only.
No future implementation was inspected. Public Reader remains unchanged.
Reserve `tests/internal/dtd_reader_spec.rs` for the later blind suite, after
the private interface and position rule below are frozen.

## Public baseline and evidence

[`Reader<'input>`](../../src/reader.rs) returns `Event<'input>` from
`read_event(&mut self)`. Its output lifetime belongs to the caller's input,
not to the mutable reader loan. A retained event can coexist with another read,
survive namespace scope unwind, and survive reader destruction. The caller must
keep the original input alive and immutable while an input-borrowing event is
used. Events can be collected into a Vec and returned after the reader drops.

[`Event`, `Attribute` and `Declaration`](../../src/reader.rs) have different
ownership details. Unchanged text, CDATA, comments, PI strings, attribute values
and lexical names borrow physical input. Normalization produces owned Cow values.
Element attribute vectors own their metadata. Declaration fields are plain
input-borrowed `&str`, not Cow. Resolved namespace strings can borrow stable input
or own budgeted copies; they must remain valid after parser bindings unwind.
[`Name` getters](../../src/name.rs) borrow the Name view: a getter result cannot
outlive its Name merely because the original lexical spelling came from input.

The constructor validates the entire input length, encoding, UTF-8 and XML
characters before returning a reader. For example, `<r/>` followed by invalid
UTF-8 or NUL fails construction at byte 4; it never publishes the valid root
first. InputBytes wins over those errors when the complete input exceeds its
ceiling. UTF-8 BOM is accepted, costs three input bytes, and sets initial position
to 3 without setting an event origin.

UTF-16 remains rejected. Existing diagnostics depend on the signature:

| Input prefix | Constructor error |
|---|---|
| UTF-16LE or BE BOM | UnsupportedEncoding at 0 |
| BOM-less BE `00 3c 00 3f` | UnsupportedEncoding at 0 |
| BOM-less LE `3c 00 3f 00` | InvalidCharacter at 1 |
| BOM-less BE `00 3c 00 72` | InvalidCharacter at 0 |

Preserve rejection, including these regression inputs, unless architecture
explicitly scopes a separate owned-event decoding path. A decoded UTF-16 buffer
inside the reader cannot back `Event<'input>` after reader destruction. The
plain borrowed Declaration fields make an implicit owned-decoding fallback
especially easy to get wrong.

Eight independent temporary public-control tests passed. They checked dynamic
input lifetimes, reader mutation/drop, namespace unwind, owned extraction,
occurrence-specific pointers, cursor/error tables, constructor precedence,
UTF-16 rejection, and exact borrowed-parser budgets. The 13 existing independent
[borrowed-controller specification groups](../../tests/internal/dtd_borrowed_spec.rs)
also passed. Evidence is under `/private/tmp/`:

- `yaxml-dtd-stable-reader-audit-controls.log`: eight passed.
- `yaxml-dtd-stable-reader-borrowed-controls.log`: 13 passed.
- `yaxml-dtd-stable-reader-audit-probes.rs`: reproducible public-control source.
- `yaxml-dtd-stable-reader-negative-input-lifetime.log`: expected E0597.
- `yaxml-dtd-stable-reader-negative-input-mutation.log`: expected E0502.
- `yaxml-dtd-stable-reader-negative-name-view.log`: expected E0597.

The three negative probes failed for the intended live source/view borrow,
not privacy, unresolved names, or a loan tied to Reader. Their source snippets
are retained beside the logs. Temporary test code is not part of this commit.

## Ordinary positions, errors and ceilings

Stable Reader text does not consume its terminating `<`. For
`<r>x<n/>x</r>` the five event ranges are `0..3`, `3..4`, `4..8`, `8..9`,
`9..13`; positions are **3, 4, 8, 9, 13**. Both `x` values must borrow their
own occurrence, not an equal string found elsewhere in the document.

EOF repeats None. Errors repeat the exact first Error and preserve position and
the last delivered origin. Before any delivery the origin and event range are
None. Normalization changes values without changing their physical input span;
an event range is provenance, not a serialization of the normalized event.
See [SourceOrigin](../../src/origin.rs) and
[public provenance tests](../../tests/source_origin.rs).

The following cursor differences are observable and passed independent controls:

| Input | Failure | Offset | Position after failure | Last event range |
|---|---|---:|---:|---|
| `<r></s>` | MismatchedEndTag | 3 | 3 | `0..3` |
| `<r>&bad;</r>` | UnknownEntity | 3 | 8 | `0..3` |
| `<r>&#0;</r>` | InvalidReference | 3 | 7 | `0..3` |
| `<r a='&bad;'/>` | UnknownEntity | 6 | 0 | None |
| `<r><!--a--b--></r>` | Syntax | 3 | 3 | `0..3` |
| `<?xml?><r/>` | InvalidDeclaration | 5 | 7 | None |
| `<?xml version='1.x'?><r/>` | InvalidDeclaration | 6 | 21 | None |
| Four spaces then `<r/>`, token ceiling 3 | TokenBytes | 0 | 4 | None |

Do not replace these with streaming failure positions on DTD-free input.
Structural/tag failures can leave the raw token unconsumed; normalization and
declaration failures can consume it. Leading/trailing whitespace is skipped but
still checked against the token ceiling. An exact event ceiling permits EOF
and skipped top-level whitespace. Whitespace inside the root is a real Text
event. `<r>&amp;</r>` needs exactly **5** owned bytes in stable Reader; ordinary
streaming needs an additional retained root-name byte. Unnormalized borrowed
names and namespaces can parse with OwnedBytes zero. Reusing streaming budget
minima would reject valid stable input.

Reader has no source extraction method, I/O error, cancellation callback or
public acknowledgement protocol. Dropping it validates nothing further.
Retained events remain usable; incomplete XML need not be diagnosed unless the
caller reads through EOF. An `into_inner` method is not needed merely because
the buffered adapter has one.

## Decisions required before implementation

1. **Return owning metadata.** A suggested private event wrapper is
   `Xml(Event<'input>) | Dtd(TakenDtdEvent<'input>)`, returned by value with
   `read_event(&mut self)`. Retain Reader-style origin/range getters or return
   an owned SourceOrigin beside it; freeze which. Current
   [`LocatedEvent`/`DtdEvent`](../../src/stream/dtd.rs) is insufficient:
   DtdEvent::Item and DoctypeHeaderView borrow a controller-owned DtdUnit.
   [`TakenDtdEvent`](../../src/dtd_controller.rs) moves unit metadata while its
   physical raw Cow can still borrow input. A retained unit's `view()` borrows
   that unit, not the vanished controller.
2. **Freeze constructors and errors.** Prefer input bytes plus Limits and
   SessionLimits, returning a Result after the existing complete-input
   preflight. Keep a typed private DTD/content error (SessionError is an
   available candidate), exact first-error stickiness and truthful physical
   offsets. Declaration-local/default diagnostics must not become document
   offsets accidentally. Public DTD rejection and public Error stay unchanged.
   Optional private DecodeLimits may constrain UTF-8 byte totals without
   implying UTF-16 acceptance.
3. **Approve the DTD position policy below.** Ordinary Reader and the accepted
   DTD coordinator have different lookahead contracts. They cannot both be
   called the same cursor without an explicit rule.
4. **Freeze Text segmentation and synthesized ownership.** Recommend the
   accepted session's separate prefix/replacement/suffix Text events, no event
   for an empty GE, and owned generated/defaulted strings. Static reserved XML
   namespace constants may remain borrowed. Do not merge different occurrences
   into input-borrowed text. Any later Mixed Text joining is a separate policy
   with its own allocation and origin rules.

### Recommended DTD-mode position rule

Preserve the exact public Reader cursor on DTD-free input. Once DTD mode is
entered, expose the accepted primary-source high-water cursor, including
already accepted GE spelling and `<` lookahead. This preserves existing
[content-controller ordering](dtd-content-controller.md) without inventing a
second committed cursor. Generated bytes never advance it. This is a deliberate
private DTD-mode distinction, not a change to the public DTD-free Reader.
Architecture must approve it or specify the alternative cursor before blind
implementation tests are written.

For `<!DOCTYPE r><r/>`, Begin's origin is `0..11`, End's is `11..12`,
and Empty's is `12..16`; recommended positions are **12, 12, 16**. The moved
Begin DtdUnit raw spelling is `<!DOCTYPE r>` (`0..12`), so its logical Begin
origin is not the complete raw-unit slice. Borrow proofs must use the sealed
raw-unit window, not blindly use the Begin event's shorter origin.

For this 64-byte fixture:

```xml
<!DOCTYPE r [<!ENTITY e '<n/>x'><!ENTITY z ''>]><r>a&e;b&z;c</r>
```

| Event | Exact input range or replacement local range | Physical reference context if generated | Recommended position |
|---|---|---|---:|
| Begin | `0..13` | — | 13 |
| ENTITY e | `13..32` | — | 32 |
| ENTITY z | `32..46` | — | 46 |
| DTD End | `46..48` | — | 48 |
| Start r | `48..51` | — | 51 |
| Text a | `51..52` | — | 55 |
| Empty n | local `0..4`, input None | `52..55` | 55 |
| Text x | local `4..5`, input None | `52..55` | 55 |
| Text b | `55..56` | — | 59 |
| Text c | `59..60` | — | 61 |
| End r | `60..64` | — | 64 |

With a UTF-8 BOM add three to physical positions, ranges and reference
contexts; replacement-local ranges stay unchanged. Before publishing Text a,
the e reference may be classified/read but no GE lookup, attempt charge or
frame entry may occur. Subsequent work follows delivery on the next read.
Held no-subset End likewise precedes body processing. EOF must drain generated
work before final parser validation.

## Blind test matrix

| Family | Concrete input or operation | Required independent oracle |
|---|---|---|
| DTD-free parity | Existing public Reader fixtures, Unicode names, declaration spelling, BOM, normalized values | Same typed events, errors, positions, ranges, ownership and quota boundaries; keep ordinary direct scanner unchanged |
| Physical borrowing | Duplicate equal Text at separate positions; lexical names/attribute values; comments/CDATA/PI | Borrowed Cow and exact source-substring pointer where representable; preserve every event across later calls and reader drop |
| Retained DTD metadata | No-subset fixture and physical ENTITY/ATTLIST/PI/comment units | Move units, retain all through subsequent units, EOF and reader drop; physical raw pointer belongs to its exact source occurrence |
| Generated body/default | `<!DOCTYPE r [<!ENTITY e '<n/>𝄞'><!ATTLIST n a CDATA '🙂'>]><r>α&e;β</r>` | Empty n has `a=🙂`; generated Text is 𝄞; owned generated/defaulted fields; replacement locals `0..4` and `4..8`; physical reference `69..72`; entire UTF-8 source length 78 |
| Generated DTD | `<!DOCTYPE r [<!ENTITY % p '<!--a&#13;b-->'>%p;]><r/>` | Generated comment unit raw is `<!--a\rb-->` with actual CR, owned raw; Replacement source; retain after controller/reader drop; no second physical EOL normalization |
| Default ordering | Accepted multiple-ATTLIST first-binding fixture | Explicit attributes retain lexical order; effective defaults append in declaration order; later duplicate declaration does not replace first; #REQUIRED is not validated and explicit #FIXED mismatch stays accepted under this nonvalidating profile |
| Namespace defaults | `<!DOCTYPE p:r [<!ATTLIST p:r xmlns:p CDATA 'urn:p' p:a CDATA 'v'>]><p:r/>` | Name p:r and default p:a resolve to urn:p, xmlns:p to XMLNS; default fields survive scope unwind and reader drop; explicit xmlns override wins |
| Expanded duplicates | Defaults introduce p/q bindings to same URI and q:a while instance specifies p:a | DuplicateAttribute, no element publication, preserved last DTD origin, exact sticky typed failure; freeze declaration-location mapping |
| Lexical DTD matching | Declaration for p:r with instance q:r bound to same URI | No defaults applied by expanded-name alias; unused malformed QName default is not eagerly applied to another lexical element name |
| Attribute normalization | Declared NMTOKENS values with numeric TAB and literal TAB/spaces | Numeric TAB survives; only U+0020 collapses after physical normalization; explicit/default `  &#x9;  x \t y  ` becomes actual TAB followed by ` x y` |
| GE phase/security | Empty/nested GE, numeric/predefined spelling, escaped ampersand, comment/CDATA/PI delimiters, unbalanced replacement | Exact event sequence without omissions/duplicates or rescanning numeric output as markup; frame boundary errors before invalid delivery |
| Occurrence identity | Outer entity expands two uses of the same inner entity | Equal local ranges and physical outer reference do not alias invocation identity; origins differ even if logical values match |
| EOF and early drop | `<r>tail`; incomplete DTD; drop after Begin or prefix Text; completed EOF repeated | EOF Text once then missing-end error; partial DTD typed error; no validation/GE scheduling during drop; no new event or origin at repeat EOF |
| Quotas | Below/exact/above each Limits and SessionLimits family; PE then GE attempts; many retained defaulted/generated events | Whole-input preflight, raw/decoded distinctions, exact DTD-event counting, same parser Budget/catalog/frame counters across handoff, no refund after dropping events |
| Borrowed-copy accounting | Zero physical raw-copy ceiling with valid physical DTD; generated unit under same ceiling | Borrowed physical raw costs no promotion bytes; grammar/family error precedes copy failure; generated raw requires its actual copy budget |
| Unread/external declarations | Accepted catalog/controller standalone and unavailable-dependency fixtures | Preserve typed non-success distinctions and effective declaration rules; no resolver or I/O silently added |
| Growth and destruction | Deterministic many declarations/defaults, Unicode names, repeated scopes; retain early events | Metadata/catalog growth and namespace rehash never invalidate early events; bounded iterative progress and destruction |

The defaulting, matching and nonvalidating policies above come from
[accepted tag specification tests](../../tests/internal/dtd_tag_spec.rs) and
[catalog policy](dtd-catalog.md). They are not new validation promises.

## Compile probes and implementation traps

The future private suite must compile-pass a function returning a Vec of
`StableEvent<'input>` after the reader is destroyed. Use a dynamically allocated
String outside that reader scope, not a static literal. Keep the first event
live across another read, namespace unwind and reader drop, then inspect it.
Run separate probes for physical XML, physical DTD units, defaulted attributes,
generated markup and generated DTD units. A lending return type accidentally
tied to `&mut self` must fail these positive probes.

Compile-fail probes must reject an event escaping its original input (E0597),
source mutation while a retained event is still used (E0502), and a Name/unit
getter view escaping its owning Name/unit (E0597 or equivalent). Require those
specific borrow errors, not arbitrary compilation failure. Unlike buffered
lending, holding an event across the next read is a required compile-pass.
Move owned normalized/generated fields out and prove those fields can survive
both reader and input destruction; do not claim that the generic outer
`Event<'input>` automatically becomes `Event<'static>` at runtime.

The accepted ParserState has the needed lifetime parameter, but current owned
ContentController uses `ParserState<'static>` and lends copied tokenizer
scratch. Returning scratch as input-borrowed would require an invalid lifetime
extension. A stable publication must select the exact validated range in its
sealed original UTF-8 window and parse once against that slice. Equal string
content, a catalog lookup result, a movable String, and a unit view are not
proofs of original-input lifetime. Generated/default data must move or copy into
event-owned storage under the same Budget, never borrow catalog or frame scratch.

Do not cache Event borrowing its own reader/controller storage, widen lifetimes,
leak allocations, clone catalog state, reset frame nonce/counters, or copy whole
events merely to escape a callback. Move DTD metadata out with the accepted
take-current boundary. Preserve the same parser, declaration/standalone state,
catalog and frames at handoff. Retained physical raw is free of promotion-copy
charges, not of scanning, metadata, catalog, work or normalization costs.

The accepted stable Text coordinator still copies physical tokens into scratch
([documented here](dtd-text-reference-adapter.md)); returning original source
slices is not a zero-copy lexing or performance claim. Whole-source fuzz,
matched performance, public DTD activation and ordered DOM/Writer support remain
separate gates.
