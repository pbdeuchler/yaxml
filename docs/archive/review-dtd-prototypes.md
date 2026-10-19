# Private DTD prototype review

Reviewed source freeze: `3e493e4d37e21b2c29c74bb2a9bdedbe2503d4f3`.
Source tree: `2e3e0d9e4439250d3ecd5b0413e645440e68d320`.
Private integration: `/private/tmp/yaxml-dtd-integration`, branch
`integrate/dtd-prototype`. Main's review base is `457b6c9`, with the same production
source tree as the validated source-origin milestone. Recording these prototype
plans and evidence on main does not integrate the private source modules.

Verdict: the grammar and scanner are suitable foundations for the next private
semantic/controller stage. This is not approval to enable DTD acceptance or
merge test-only staging into main. The combined ordinary checks passed on this
exact clean source freeze; no new fuzz-coverage or performance claim is made.

## Scope reviewed

The unit parser validates one complete decoded XML 1.0 header or declaration,
comment or PI. It retains one borrowed/owned raw string and a flat token vector;
typed views borrow the unit and never reparse declaration text. ELEMENT models
and their destruction are iterative. ATTLIST enumeration alternatives use
Nmtoken grammar while NOTATION alternatives use Name grammar. Context, binding,
normalization, resolution and validity checks remain outside this constructor.

The scanner frames one bounded decoded unit at a time. It preserves quote and
delimiter state, validates completed UTF-8 and PE names, seals input at final
EOF, retains ready units until release, preserves the body suffix, and makes
failures sticky. Its offsets and work are decoded-local, not physical source
coordinates. Its work ceiling counts consumed bytes; the fixed framing and
final validation passes remain linear in that count.

Both modules are intentionally wired only under `cfg(test)` on a private branch.
The shared NameChar predicate was factored from existing name validation;
there is no grammar or namespace behavior change in current public parsing.
The prototype adds no dependencies, unsafe code, external I/O or public events.

## Findings resolved before freeze

- A reference-name parser converted token-limit and allocation errors into
  InvalidReference. It now maps only lexical InvalidName failures, preserving
  quota/allocation failures. A regression exhausts the quota inside both
  `&name;` and `%name;`, before the complete reference token can be stored.
- The first unit representation was 112 bytes, with a 64-byte kind. Removing
  redundant stored indexes and selecting validated fixed token positions
  reduced the unit to 56 bytes and its family tag to one byte. The token remains
  64 bytes. No unchecked construction was introduced.
- That simplification initially made ATTLIST definition_count linear. A cached
  count in the existing root token restores constant-time access without any
  layout increase.
- Comment rejection initially identified the first hyphen instead of the
  second impossible hyphen. Fixed along with partial-keyword/EOF diagnostics.
  The independent BOGUS dispatch expectation was updated from zero to byte two
  after the explicit longest-prefix diagnostic contract was implemented.
- Scanner finalization initially permitted resumed input after draining trailing
  whitespace. Final EOF now seals advance while allowing ready-unit draining.
- Incomplete PE references at EOF now report already-known Name errors before
  missing-semicolon EOF, with invalid UTF-8 retaining precedence.
- Author tests were moved out of the independent catalog's reserved file.
  Both suites remain separate and are compiled together.

## Review dimensions

| Dimension | Assessment |
| --- | --- |
| Correctness | All declaration families, delimiters, reference forms, validity-only acceptance controls and exact limits have literal tests. Four independent pipeline groups compose scanner output with unit validation. |
| Security | Unit bytes, token count, model depth and definition count are checked before growth; scanner unit/scratch/work ceilings are independent. Reservations are fallible, parsing is iterative, arithmetic is checked or bounded by existing vector lengths. |
| Ownership/API | Raw owned buffers move; borrowed buffers remain borrowed. Views borrow the unit, fields cannot be forged, and no parser-owned value is mislabeled as an input borrow. |
| Simplicity | Concrete unit parser, flat metadata and explicit scanner states; no resolver framework, recursive models, global arena or second raw DTD tree. |
| Idiomatic Rust | Sum types distinguish grammar forms; Cow retains borrowing, iterator views avoid field allocation, and try_reserve converts allocation failures. |
| Maintainability | Scanner framing, unit grammar and future document semantics have explicit boundaries and separate plans/tests. Private staging is clearly identified. |
| Verification | Author and independent catalogs, all-split/one-byte Unicode feeds, bounded generated cases, typed-value assertions and deliberate runtime faults test distinct contracts. |

No unresolved correctness finding blocks the next private integration stage.
Actual allocation exhaustion has not been injected. Unit token storage is still
64 bytes per token and should be profiled with realistic DTDs before tuning.
The measured existing Event remains 88 bytes; the eventual public DTD event
variant has not yet been added and must receive its own layout measurement.

## Combined verification

Fresh root checks passed 585 tests with nine ignored across 44 ordinary result
groups, including all 50 new focused groups: 13 author grammar, 14 independent
grammar, 19 scanner and four independent pipeline groups. The existing 64
fuzz-smoke tests are part of that ordinary suite; they do not target the private
DTD modules. All five existing benchmark smoke binaries ran, with no comparative
timing claim. All 13 doctests passed (eight runnable and five compile-fail), as
did strict all-target Clippy, formatting and diff checks.

Root runner `/private/tmp/yaxml-dtd-root-final-checks.sh` verified the exact clean
Git head before and after. Its handle `24754` is terminal with exit zero.
Logs use `/private/tmp/yaxml-dtd-root-final-{all-targets,doctests,clippy,fmt,diff}.log`.
The root also compared all 286 tracked files byte-for-byte with their Git blobs.
Compiler: Rust/Cargo 1.95.0, LLVM 22.1.2, aarch64-apple-darwin.

## Deliberate faults

Root checks caught six grammar faults after independently passing each clean
baseline: bypassed XML characters, collapsed reference quota classification,
mixed group connectors, rejected PUBLIC-only notation, bypassed model depth and
infallibly promoted borrowed raw input. Every mutant failed a runtime assertion;
a compile failure was not accepted as evidence. Final source was restored.

Root checks also caught five scanner faults: lost declaration quote context,
premature subset closure, CR renormalization, unsealed EOF and shifted UTF-8
offsets. An independent reviewer separately caught work-boundary, EOF-sealing,
quoted-delimiter, UTF-8-validation and missing-model-particle faults. The last
fault was caught by the scanner-to-grammar pipeline's malformed-unit gate.

Evidence:

- `/private/tmp/yaxml-dtd-root-unit-faults.tsv` and associated baseline/fault logs;
- `/private/tmp/yaxml-dtd-root-scanner-faults.tsv` and associated fault logs;
- `/private/tmp/yaxml-dtd-scanner-review.md`, SHA-256
  `f8ec38ea5a48d371c4375ed6b78e9356151eb0363e4afb9d52c3a9c2e3c1bb7a`;
- `/private/tmp/yaxml-dtd-layout-de19d78.tsv` and
  `/private/tmp/yaxml-dtd-layout-caca791.log`.

## Activation gates remain open

The runtime controller does not yet exist. It must enforce the family/context
gate demonstrated by the test helper: a framed Declaration cannot be treated as
a nested DOCTYPE header. UTF-8 framing also does not establish XML character
validity; the unit constructor performs that separate check.

Next work must connect first-binding GE/PE/attribute tables, processed replacement
text, default expansion and declared-type normalization, unread-entity context,
iterative source frames and occurrence provenance. Generated events require
fallible ownership promotion. Both DOMs require parse-only text coalescing and
honest mixed provenance. Physical decoder/I/O progress remains separate from
replacement-local ranges and reference error/cancellation contexts.

Only the coherent controller may enable DTD processing across all five frontends.
That activation must remove test-only module wiring, add public event/DOM/writer
support, source-complete fuzz targets and coverage denominators, complete XMLTS
deltas, serialization checks, layouts, paired performance evidence and another
independent review. Current main's coverage and conformance measurements do not
measure this private prototype. Full compatibility, 100% fuzz coverage and
performance superiority remain open goals.
