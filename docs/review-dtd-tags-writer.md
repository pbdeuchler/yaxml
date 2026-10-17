# Private DTD tag, content foundation and Writer review

This continues the [controller review](review-dtd-controller.md). The reviewed
source remains on `integrate/dtd-prototype`; main retains production source
tree `28ac931425855790f29b6ab25fa6683e5fcc3aa7`. Public DTD acceptance, generated
content scheduling, both DOMs' DTD storage and serialization, fuzzing and full
conformance gates remain open.

## Start tags and atomic namespace publication

The [tag preparation outline](plans/dtd-start-tag-preparation.md) separates raw
literal collection, catalog normalization/default insertion, and namespace
publication. Explicit attributes retain their order and precede defaults.
Defaults can declare namespaces used by the element and other attributes.
First declarations win; validity constraints such as REQUIRED and FIXED remain
outside this nonvalidating slice.

Publication prepares owned strings, namespace IDs, pending bindings, expanded
name checks and stack capacity before committing logical scope. Rejection
leaves namespace membership, the next ID and element frames unchanged. Work and
allocation attempts remain spent. Two fallible pending maps avoid quadratic
prefix/URI searches; capacity reservation follows actual xmlns declarations.

Root review and independent tests found and corrected:

- Input-relative attribute normalization errors needed rebasing to the literal's
  actual tag offset; declaration and replacement locations must retain their
  separate coordinate systems.
- Namespace updates could escape a rejected tag. Pending publication now
  commits only after preparation succeeds.
- A required pending-map insertion inside `debug_assert!` disappeared in release.
  The insertion now runs unconditionally; both profiles have regressions.
- Normalized value length understated cumulative allocation requests. The
  bounded catalog operation now checks requested growth before reserving and
  returns the debit even on a later error. The initial 64-byte quantum is not
  clamped to the caller's allowance. Native value-length and caller allocation
  failures remain distinct.
- An invalid default QName reported its name-relative offset as declaration
  relative. The catalog now retains the original name token offset; ASCII and
  multibyte UTF-8 cases assert exact declaration-byte coordinates.

There are 21 author (including four route controls) and 17 independent tag groups, including exact/one-short
budgets, rejection recovery, lexical and expanded duplicates, default namespaces
and generated-name ownership. Root deliberately broke seven final-state
behaviors; named controls rejected every fault before source restoration.

## Ordinary parsing performance

Routing every ordinary tag through staging caused large avoidable costs. A
pinned release diagnostic used four fixtures, five APIs, five alternating pairs
per workload and identical semantic observations: 100 pairs, 200 rows. Every
one of the 20 median parse times regressed, by 16.37–96.42%. Attribute DOM parsing
was worst at 96.42%; namespace DOM parsing regressed 53.72%.

The [fast-path correction](plans/dtd-tag-fast-path.md) restores the shipped
one-pass ordinary algorithm. Only callers holding completed DTD state use the
shared `process_dtd_start` orchestration. It checks parser ceilings, collects,
normalizes, publishes and commits one event. This supersedes the initial tag
outline's proposal to stage DTD-free parsing. No policy object or optional
catalog enters the ordinary hot path.

The same diagnostic compared baseline `304ab59` (source `2a911ccbe262e2f8f4c78e914e49795576cb7591`)
with combined correction `0b38ca5` (source `8a170f0417fcf98e566f1e89c2e4ad09079dbc20`).
All 100 pairs retained identical bytes, counters and digest. Eighteen workload
medians ranged from -1.58% to +0.24%. Two initial outliers received a retained
nine-pair targeted repeat with crossed controls:

| Workload | Full matrix change | Targeted repeat change |
| --- | ---: | ---: |
| Flat DOM | +4.67% | +0.22% |
| Namespace buffered | +2.18% | +0.38% |
| Namespace DOM control | -0.15% | -0.38% |
| Flat buffered control | -0.12% | -0.46% |

All 36 repeat pairs also agree semantically. An independent audit recomputed
medians, percentages, pairing, digests and source pins. This supports removal of
the large staged-path cost. It does not establish zero regression or superiority
over libxml2. There are five samples per full workload and nine per repeat, no
formal confidence estimate, and source linkage relies on the recorded build
process. These runs precede Writer integration and are not a benchmark of that
later source freeze.

Retained scripts and metadata are under `/private/tmp/yaxml-dtd-tag-performance`:
`run.rb`, `run-fixed.rb`, `run-fixed-repeat.rb`, their logs, and the original,
`fixed-` and `fixed-repeat-` metadata, raw rows and summary files. Successful
child stderr is not retained by this diagnostic runner; it is not a strict
benchmark certification harness. Both initial outliers remain in the evidence.

## Content foundations

The [content controller plan](plans/dtd-content-controller.md) now has a private
transport-free local tokenizer and one-reference scratch scanner. They retain
distinct input versus replacement ranges, monotone aggregate work and sticky
failure/EOF behavior. Restart does not restore quotas. Numeric/predefined and
general references are classified without catalog lookup or replaying output.
Raw scanning, scratch copies and final classification are charged separately.
Both scanners cap work before operations, including final validation.

Twelve author and twelve independently derived groups exercise split points,
EOF, saved delimiters, reference classes, invalid characters, exact quotas and
restart. Three independent runtime faults were caught and restored. Measured
test-build layouts remain Tokenizer152, Stream520, Push656 and Reader392 bytes,
all alignment8. This is not an activated production-layout claim.

The [Text/reference adapter outline](plans/dtd-text-reference-adapter.md) is the
next boundary: stop Text before `&`, classify once, append raw numeric/predefined
spelling once, and release any Text prefix before entering a general entity.
Full coordinator scheduling and frame-aware generated events remain unfinished.

## Writer protocol

The private [Writer protocol](plans/dtd-writer-protocol.md) accepts ordered
logical Begin, Item and End deliveries. It holds the real closing delimiter
until End, preserves the completed catalog, applies each declaration once, and
checks cumulative unit, catalog, event, encoded-byte and work ceilings. External
identifiers remain inert. Unavailable dependencies cannot disappear behind a
duplicate declaration.

Each event is fully buffered and validated before sink I/O. Replacement literal
encoding preserves phase semantics, and unrepresentable identifier CR errors
retain separate unit and prospective output coordinates. Sink errors or panics
leave the Writer terminal; partial output positions count only accepted bytes.
Ordinary Start/Empty calls after End automatically use the shared DTD tag path.
Sparse events may omit defaults: the DTD recreates them on parsing. Explicit
values and namespace metadata must agree with the encoded event's effective
attributes. Writer does not silently normalize a caller's tokenized value.

Four state-author groups, eleven integration-author groups and twenty
independent groups cover lifecycle, literal phase, sparse/default namespaces,
native versus global limits, exact diagnostics, short/Interrupted writes,
zero/oversized writes, sink errors/panics and flush behavior. Author and
independent suites each caught five deliberate runtime faults. Root reviewed
the source and literal oracles before combining them with the fast path.

## Combined freeze and remaining gates

Freeze `a1c4bcbd116abd0dddabf9689f7e9890e49aae4c`, source tree
`5f82832ef1305f072c65b0c2cad6a318abbf82bd`, passes 885 tests with nine ignored
across 44 ordinary groups, including all 447 library tests. All 447 library
tests also pass in release, as do all 13 doctests. Strict all-target Clippy,
formatting and diff checks pass. Root compared all 330 tracked files against
their Git blobs; the worktree is clean.

Root logs use `/private/tmp/yaxml-dtd-root-writer-{library,all-targets,release,doctests,clippy,fmt,diff}.log`;
handle `33696` is terminal with exit zero. The SHA-256 source manifest is
`/private/tmp/yaxml-dtd-root-writer-source.tsv`. Earlier independent artifacts
include `/private/tmp/yaxml-dtd-writer-spec-report.txt`, its artifact manifest,
and `/private/tmp/yaxml-dtd-writer-spec-faults.tsv`; root tag fault cases are
recorded in `/private/tmp/yaxml-dtd-tag-root-final-faults.tsv`. These are retained
local artifacts, not CI-hosted or remotely published evidence. Toolchain is
Rust/Cargo 1.95, LLVM 22.1.2, aarch64-apple-darwin, with locked memchr 2.8.3 as the
sole dependency and unsafe code forbidden.

No fresh fuzz-only coverage or XMLTS result is claimed for these private
modules. Main's most recent public-source measurement remains 6,936/7,406
regions and 910/964 instrumented branch outcomes. Reaching 100%, sustained
hostile-input campaigns, allocation-failure injection, generated content
scheduling, public DTD integration, matched libxml2 performance, allocator
support and language bindings remain open.
