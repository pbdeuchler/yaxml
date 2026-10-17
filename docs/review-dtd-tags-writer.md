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

## Follow-up review corrections

Final independent Writer review found an Important validation-order issue in
the initially green checkpoint: Begin and Item could encode a full bounded
token before checking known exhausted private event/item counts. A later
encoding error could therefore precede the earlier quota failure. No sink
publication escaped and outer failure poisoning remained correct.

The correction adds constant-time Begin/Item/End count preflights before buffer
reset or encoding, retaining the core's commit-time checks. A twenty-first
independent Writer group first failed against the old source, then passed the
correction. Removing each of the three preflights separately causes a runtime
failure; all source was restored. Buffer contents, sink calls, position and
counters remain unchanged on these rejections. Retained evidence uses
`/private/tmp/yaxml-dtd-root-writer-count-preflight-{red,green}.log` and
`/private/tmp/yaxml-dtd-root-writer-count-faults.tsv`.

The next narrow Text/reference operations also now compile privately. They stop
Text before ampersands, use checked physical/local extents, append raw character
references once and require Text-prefix release before general-reference skip.
Root review caught an uncharged repeated UTF-8 scan in the validated-reference
getter. Pending bytes now transition into a checked String once during charged
classification; getters borrow it in constant time and release reuses the same
allocation. Six new author groups, the existing independent foundations and
eight new independent boundary groups pass in debug and release. The latter
use actual Decoder output at every source split for UTF-16LE/BE, including
supplementary preceding Text and Unicode general names; exact physical/decoded
ceilings, saved delimiters, local sealing and four runtime faults are covered.
The independent artifact is `4130958`; its report is
`/private/tmp/yaxml-dtd-text-reference-spec-report.md`. The source-bound
coordinator remains in progress; this is not a completed content controller.

## Owned generated body events

The [generated event preparation outline](plans/dtd-parser-event-preparation.md)
now has private leaf, Start and End operations returning `Event<'static>`.
Fields are constructed owned at their real preparation boundary; no general
Event-to-owned conversion or asserted lifetime is introduced. Physical event
borrowing retains its existing wrapper. Generated namespace output selects a
second monomorphized copy callback inside the same publication transaction,
including inherited URIs borrowed from short-lived physical input.

Replacement Text preserves literal CR/CRLF and processes numeric/predefined
spellings once, without rescanning decoded ampersands. CDATA, comments and PI
data preserve replacement CR. XML-target PIs are rejected. Root review caught
generated Text being accepted in Prolog/Epilog; the corrected body-only path
rejects even empty and whitespace-only values before spending events or copy
budget. UTF-8/XML-character validation remains the work-charged LocalTokenizer
obligation, matching the physical parser's documented precondition.

Generated End copies its qualified name and namespace before popping the exact
current frame or unwinding scope. It has no prepared token which could be
replayed against an ABA-equivalent stack. The future controller must still run
the EntityFrames boundary guard before this call. Error offsets are byte
positions within the active replacement source, including the token's start
once; they never enter a physical UTF-16 mapper.

Six new author parser groups and eleven root independent groups cover final
values after source destruction, exact/one-short copy quotas, Unicode errors,
defaults, inherited and pending namespaces, mismatch recovery and End failure
after namespace copying but before qualified-name copying. Final review found
that the first tests exercised only generated Empty tags. Two added independent
groups now prove nonempty Start retention after raw/catalog destruction, later
child resolution, owned End and scope unwind. The exact 29-byte case succeeds;
one byte short rejects the final three-byte frame-name copy, keeps 26 bytes
spent, and leaves depth, events and pending namespace membership unchanged.

All eleven independent groups pass debug/release and strict checks. Root's
pre-correction phase test failed at runtime; three later deliberate EOL,
namespace-budget and premature-pop faults were caught and restored. Independent
holistic review of final source `d85de448` and tests `f490653` passes with no
remaining findings. Reports/logs include
`/private/tmp/yaxml-generated-body-event-review.md`,
`/private/tmp/yaxml-dtd-owned-leaf-root-nonempty-{debug,release,clippy}.log`,
`/private/tmp/yaxml-dtd-owned-leaf-root-before-phase.log` and
`/private/tmp/yaxml-dtd-owned-leaf-root-faults.tsv`.

The next source-lease coordinator remains outside this freeze. Its initial
review found a missing aggregate scratch ceiling; the correction must include
the paused physical tokenizer as well as active local/reference buffers.
Stable-input lexing still copies tokens into scratch, and borrowed stable-window
publication is not implemented. Full GE scheduling, frame guards, catalog
borrows, global counters and origin publication must now be joined before any
frontend activation.

## Source coordination and work-ledger follow-up

The corrected coordinator now reserves the configured Input and Local token
slots plus effective reference scratch together. The paused Input allocation
remains reserved while replacement input runs. Adoption checks retained capacity
and configured growth requests before consuming handoff permission; checked sum
overflow and short scratch quotas remain distinct. This is a requested-capacity
policy, not a bound on allocator rounding, RSS or the entire DTD session.

Nine independent groups cover actual UTF-16 decoding at every physical split,
supplementary prefixes, raw character-reference replay, prefix acknowledgement,
foreign stable feeds, unread EOF, invocation identity, native/global work and
scratch formulas. A retained-capacity case actually grows and releases UTF-16
tokenizer scratch before reconfiguring UTF-8. It does not manufacture a Vec with
the desired capacity. Debug/release and strict checks pass; three deliberate
prefix, UTF-16-width and scratch-formula faults were caught and restored.
Evidence: `/private/tmp/yaxml-dtd-text-coordinator-spec-report.md`.

The shared ledger now admits external catalog/frame work through sticky-first
remaining allowance and checked charges. A disjoint token/ledger borrow permits
charging before a callback while its physical event borrows token scratch. The
same ledger moves through source handoffs without restoring allowance. The
UTF-8 resume constructor requires a pristine tokenizer and explicit cursor pair;
the body owner must independently establish the retained encoding.

Root's independent near-`usize::MAX` resume test exposed a real arithmetic panic
on a following plain byte. The final bridge checks accepted Input prefixes
before tokenizer advancement and returns typed sticky `PositionOverflow` while
preserving accepted-prefix accounting. UTF-16 continuation bytes with zero
physical width remain valid. The independent regression requires unchanged
cursors/work at exhaustion and identical failure from advance, finish and
remaining allowance. All eight bridge groups in its separate review worktree
pass debug/release and strict checks. Author faults also detect dropped debit,
sticky-error bypass and removal of the position guard.
Evidence: `/private/tmp/yaxml-text-work-ledger-report.md` and
`/private/tmp/yaxml-text-resume-overflow-root-review.md`.

The complete body-controller composition is a subsequent private slice under
review. Stable-window borrowed publication and general UTF-16 body resume remain
absent. Stable-mode lexing still copies physical tokens to scratch; no DTD-aware
Reader performance claim follows from these tests.

## Combined freeze and remaining gates

Initial freeze `a1c4bcbd116abd0dddabf9689f7e9890e49aae4c`, source tree
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

The corrective follow-up freeze is `759aad03a8cb36f9d22e7fdbeeeb35ffa76200cf`,
source tree `9114ceea69408bd628124dde9d0fc8bb02d2a1a3`. It passes 892 tests with
nine ignored across 44 ordinary groups, all 454 library tests in release, all
13 doctests, strict all-target Clippy, formatting and diff checks. All 332
tracked files match their Git blobs and the worktree is clean. Root handle
`72621` is terminal with exit zero; logs use
`/private/tmp/yaxml-dtd-root-count-final-{all-targets,release,doctests,clippy,fmt,diff}.log`
and the source manifest is `/private/tmp/yaxml-dtd-root-count-final-source.tsv`.

The owned-event follow-up freeze is `cf6654c4cd4dfeb15ae7bf90222bda6f5e254997`,
source tree `3342eb81c9b5613feb98defe2c94dd8a74a32c07`. It passes 917 tests with
nine ignored across 44 ordinary groups, all 479 library tests in release, all
13 doctests, strict all-target Clippy, formatting and diff checks. All 335
tracked files match their Git blobs; the worktree is clean. Root handle
`63437` is terminal with exit zero. Logs use
`/private/tmp/yaxml-dtd-root-owned-events-{library,all-targets,release,doctests,clippy,fmt,diff}.log`;
the earlier library-only log contains 477 tests, before the two nonempty-Start
controls were added. The final all-targets/release logs contain all 479. Source
manifest: `/private/tmp/yaxml-dtd-root-owned-events-source.tsv`.

The coordinator/ledger freeze is `7d66aa502201cdb8e6745bc6c97399a56c7d1bd5`,
source tree `12ca5cbaa43f685284bda205f274879d6b7aeee0`. It passes 942 tests with
nine ignored across 44 ordinary groups, all 504 library tests in release, all
13 doctests, strict all-target Clippy, formatting and diff checks. All 339
tracked files match their Git blobs and the worktree is clean. Root handle
`23795` is terminal with exit zero. Logs use
`/private/tmp/yaxml-dtd-root-text-ledger-{all-targets,release,doctests,clippy,fmt,diff}.log`;
source manifest: `/private/tmp/yaxml-dtd-root-text-ledger-source.tsv`.

No fresh fuzz-only coverage or XMLTS result is claimed for these private
modules. Main's most recent public-source measurement remains 6,936/7,406
regions and 910/964 instrumented branch outcomes. Reaching 100%, sustained
hostile-input campaigns, allocation-failure injection, generated content
scheduling, public DTD integration, matched libxml2 performance, allocator
support and language bindings remain open.
