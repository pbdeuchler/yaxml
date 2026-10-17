# Entity text boundaries and provenance

Decision for the pending internal-DTD implementation, based on main
`f2bd0438804660f8b9004e59743011f45e85d50d`. This does not enable DTD parsing.

Use bounded, source-homogeneous Text events in Reader, push and buffered pull.
Coalesce immediately adjacent Text siblings in both parse builders. Keep CDATA,
comments, PIs and elements as node boundaries. Construction and Editor APIs
retain their existing node identities, including deliberately adjacent Text.

## Compatibility evidence

The pinned libxml2 2.15.4 expanded DOM and xmlReader coalesce adjacent text across
entity boundaries. SAX exposes fragments; their sizes also depend on feed size.
The native streaming contract deliberately differs from xmlReader's Text-node
granularity. An exact xmlReader adapter would need an explicit logical-text
ceiling before buffering a complete expanded text node.

| Physical content | Entity replacement | Expanded DOM root children |
| --- | --- | --- |
| `pre&e;post` | `tail` | Text(`pretailpost`) |
| `pre&e;post` | empty | Text(`prepost`) |
| `pre&e;post` | `<a/>tail` | Text(`pre`), Element(`a`), Text(`tailpost`) |
| `pre&e;post` | `head<a/>` | Text(`prehead`), Element(`a`), Text(`post`) |
| `pre&e;post` | `<![CDATA[c]]>tail` | Text(`pre`), Cdata(`c`), Text(`tailpost`) |
| `pre&e;post` | `<!--c-->tail` | Text(`pre`), Comment(`c`), Text(`tailpost`) |
| `pre&e;post` | `<?p d?>tail` | Text(`pre`), PI(`p`, `d`), Text(`tailpost`) |

Two adjacent literal CDATA sections remained two nodes. An empty entity alone
produced no Text node. Direct numeric/predefined references split SAX callbacks
but remained part of one DOM/Reader Text node. Nested entities and Unicode text
preserved the same topology rules. Start/end tags crossing entity boundaries
were rejected; successful prefix callbacks remained observable before failure.

These are measured libxml2 behaviors and an explicit Rust API policy. XML's
[character delivery](https://www.w3.org/TR/2008/REC-xml-20081126/#sec-white-space)
and [parsed-entity nesting](https://www.w3.org/TR/2008/REC-xml-20081126/#sec-parsed-ent)
requirements do not prescribe this event partition or DOM storage topology.

## Streaming and DOM implementation

Split Text on general-entity frame entry/exit and ordinary markup boundaries,
independently of feed size. Numeric/predefined references stay in the containing
physical lexical run. Emit no empty Text for an empty entity. Bound each run
and account for expansion work, depth, reference attempts and bytes separately;
empty entities still cost work. Never buffer the whole expanded document or an
unbounded logical Text node to match xmlReader granularity.

For `e="<a/>tail"`, emit Text(`pre`), Empty(`a`), Text(`tail`), Text(`post`).
The final two fragments have different sources and become one parsed DOM Text.
The borrowing builder can adopt an owned incoming value or promote a borrowed
prefix once, using fallible geometric growth charged before mutation. Avoid
infallible `Cow::to_mut()` and repeated prefix copying. The owned builder can
extend a text range at its pool tail; any fallback must preserve amortized
linear copying. Merging creates no new node slot and must preserve sibling and
subtree bookkeeping. Report allocation/quota failure at the incoming fragment's
real diagnostic context.

## Provenance rules

An exact replacement origin needs a fresh invocation identity, replacement-local
coordinates and a physical reference context. Declaration identity alone is
insufficient when an entity is invoked twice. Keep that metadata self-contained;
streaming must not retain a historical arena of all expansion occurrences.

Contiguous fragments of the same exact source/occurrence may join their ranges.
Different sources, distinct occurrences, source gaps or an already mixed origin
produce an explicit Mixed summary. Parsed Mixed stays `Some(origin)`, distinct
from the absence of parsed provenance on constructed owned nodes. Its
`input_range()` is `None`; it reports no single exact replacement span either.
Any retained physical causal cover must be explicitly inexact. For replacement
`<a/>tail`, `&e;post` also produced the element, so it cannot be the exact lexical
range of Text(`tailpost`). An empty entity can create a gap between two physical
text fragments; conservatively mark that merge Mixed too.

Physical decoder/transport progress, fragment-local offsets, failure context and
reference cancellation endpoints remain separate. Do not derive buffered I/O
consumption from fragment origins. Coarse origin equality cannot authorize an
element closure: retain entity-frame lexical and entry-depth checks, and require
exact-source joins. Measure actual Origin/event/node/parser layouts before
activation; the current direct-input-only Origin is not this completed model.

## Reviewed probe record

Evidence directory: `/private/tmp/yaxml-entity-text-probes`. The root review read
the helper, runner, literal topology audit, repeat script, command records,
input identities and raw representative fixture. All 125 strict invocations
and their 125 repeats passed: 23 accepted fixtures and two boundary-rejection
controls, each through DOM, Reader and SAX feeds of 1, 7 and 4096 bytes.
The root independently replayed all 125 strict commands with byte-identical
stdout/stderr and exits, and verified all 856 retained artifact sizes and
SHA-256 identities.

The helper retained default SAX2 DTD/entity callbacks, set parser-context user
data, used NOENT | NONET | NO_XXE and denied resource loading per context. Every
run recorded zero loader attempts. Both compile/runtime versions were 21504.
The exploratory `xmllint --sax` output is excluded: its custom handler reported
declared entities undefined while returning exit zero. No timing was measured.

| Retained artifact | SHA-256 |
| --- | --- |
| Current probe README | `b06cb94405f36c5500ff1cdce816b5a3bf0d4de118b4677ffd064fbec15b75aa` |
| Current artifact manifest | `4e58a19a862528118d1b490c3a5b6f61cfe54966353fe02588db02544bd84637` |
| `trace.c` | `680123fcf44d1cb9cbe9e972e8caa1800855acad4bd4dd8deb8f0270f5245590` |
| Strict helper | `0e3ee283ac7729693b39b897de9f64230fc26373e4a7eccc623d22c5211017a7` |
| Linked existing `libxml2.a` | `b48612c66e7d4d7fae1ceb7db9a0d3e288085bc91b560f037862607eccac1ab0` |

The observed numeric-CR-to-LF behavior inside an entity remains a normalization
policy discrepancy, as recorded in [DTD foundations](dtd-foundations.md).
This decision does not resolve it or authorize external I/O. Activation still
requires complete grammar, binding/default semantics, source frames, limits,
serialization, cross-interface tests, fuzzing and independent review.
