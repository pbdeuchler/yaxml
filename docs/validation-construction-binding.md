# Construction binding and limit precedence validation

The source freeze is `f9c251ac034d651189067c53169af2417450d787`, based on
main `4348e4992c479a967d1592300e156e91f94006c0`. The two author commits
`07734bf` and `092775f` were cherry-picked as `5106eb5` and `f9c251a`.
Only the fuzz model, its inventory, one corpus seed and replay registration
change. Production source remains exactly tree
`408df38ccc35a3a3f2cab1cdd5b86734818d3490`; dependencies and APIs are unchanged.

The [plan](plans/construction-binding-fuzz.md) extends the existing independent
construction model. An explicit default `v` makes automatic `{u}e` examine a
local `r=u` binding. A later exact `r:a` with no namespace makes that binding
incompatible: a two-byte name ceiling fails on generated `ns1` first, while a
three-byte ceiling reaches the later attribute namespace error. The compatible
`r:a`/`u` control succeeds by reusing `r`.

Four construction/input surfaces and three variants yield twelve literal cases.
They check exact errors/locations, names, ordered declarations/attributes,
source ranges, topology, serialization, rejected-state preservation and recovery
through the same editor. Error expectations precede each attempted operation.
Input bytes seven/eight select one fixed case; its work is independent of input
length. The nine-byte seed is `00 00 00 00 00 00 00 03 02` and selects the
UTF-16BE compatible control.

## Review and checks

The [independent holistic review](review-construction-binding.md) passes with
zero remaining findings. Initial tests did not detect removal of real dispatch
or collapse of encoded inputs to UTF-8. The final tests observe the actual
top-level driver and literal input prefixes/ranges; both faults now fail.
An independent exact-error-offset fault also fails. The author separately broke
matching-prefix compatibility in both directions; the literal negative and
positive controls caught both faults. All production mutations were restored.

Author and reviewer focused runs each passed five validation tests and the
corpus replay/truncation test. Strict test Clippy, formatting and diff checks
passed. Root inspected the complete model/follow-up diff and review, independently
probed the two error outcomes against the actual release library, and verified
that the integrated production tree is unchanged.

The first reachability probe lacked the conflicting default, so the planner
could install a default binding without examining `r`. That fixture did not
prove unreachability. The corrected fixture and fresh execution counts below
establish the actual path; no validation was weakened to reach it.

## Fresh fuzz-only coverage

Only the freshly instrumented `fuzz_smoke` executable supplied profiles.
All **64 tests passed** in 101.09 seconds, including replay and deterministic
seed mutations. The corpus has 24 files plus two inline seeds. The compiler is
nightly 2026-06-06, LLVM 22.1.6, with region and branch instrumentation. All
sixteen production files are supplied to the report; `lib.rs` has no mapped
code. No ordinary-test profiles, production exclusions or new production hooks
are included.

| Metric | Covered / total | Percent |
| --- | ---: | ---: |
| Regions | 6,885 / 7,331 | 93.92% |
| Functions | 397 / 447 | 88.81% |
| Lines | 4,756 / 4,923 | 96.61% |
| Instrumented branch outcomes | 909 / 962 | 94.49% |

The [complete source report](validation/fuzz-coverage-construction-binding-f9c251a.txt)
retains every mapped file. Seven previously missed regions/lines in construction
are now covered. In particular, the matching-prefix `None` arm at
`src/owned/construction.rs:640–641` executes 35,347 times, versus zero in the
previous run. LLVM's instrumented branch-outcome count is unchanged; a covered
match arm is not necessarily a separately counted branch outcome.

The crate-wide 100% target remains open. Remaining missed paths stay in the
denominator and require further evidence rather than an assumption that every
gap is unreachable. This finite run is not a sustained fuzz campaign or proof
of absence of security defects. No performance or new conformance claim is made:
the production parser did not change.

## Retained artifacts

Fresh recipe: `/private/tmp/yaxml-binding-fuzz-coverage-f9c251a.sh`.
Evidence: `/private/tmp/yaxml-binding-fuzz-coverage-f9c251a`, including compiler,
binary, source/harness/corpus hashes before and after, raw profile, merged
profile, complete JSON export, logs and report. The source/harness manifests
remain identical across execution, and the clean source freeze is recorded.

Author details and mutation logs are indexed by
`/private/tmp/yaxml-name-binding-fuzz-report.md`; the independent report and
its retained log hashes are copied with this validation. Detailed executable,
profile and log artifacts remain in temporary storage; compact reports are
retained here.
