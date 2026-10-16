# XML conformance runner holistic review

**Review basis:** the P0 compatibility-harness requirements in
`docs/roadmap.md`, `docs/compatibility.md`, and `docs/security.md`; the frozen
runner contract supplied for this review

**Base:** `a8bcbe2`

**Reviewed tip:** `21c746fd6e05281bda86764ee5e6dfebd2e12086`

**Scope:** eight files; 2,899 insertions; production source, Cargo inputs,
dependencies, and fuzz harnesses unchanged

## Verdict

**PASS for code quality at the reviewed boundary.** No Critical, Important, or
Minor finding remains in the pinned catalog helper, catalog parser and policy,
worker protocol, process isolation, command-line runner, integration tests, or
CI wiring.

The final clean full-suite and official-suite evidence run was still in
progress when this artifact was written. This verdict covers the frozen code,
the focused checks below, and the independently reproduced catalog manifest.
It does not claim the pending run's counts or elevate this acceptance/rejection
slice to full XML conformance.

## Findings

### Critical

None.

### Important

None.

### Minor

None.

An interim review found that the first C helper rejected `/` as a suite root
before applying its bounded path-membership rules. The frozen tip fixes the
ordering, retains the same containment checks for resolved test files, and
adds an ignored pinned-helper regression test. The final helper accepts the
official root and reproduces the previously reviewed catalog manifest exactly.

Two interim CI evidence gaps were also fixed before the reviewed tip. CI now
compares sorted start and end file inventories rather than checking only the
initial paths, and it captures, requires, and uploads an empty catalog-helper
stderr file. These were evidence-integrity findings, not parser behavior
defects.

A final review found that the honest capability-gap classification was not by
itself a sufficient regression gate. A formerly passing ID/surface pair could
begin returning `UnsupportedDtd`, `UnsupportedEncoding`, or a limit error and
leave CI green as a gap. The reviewed tip closes this finding with a strict,
checked-in required-pass inventory. Its 407 unique IDs encode 1,871 individual
surface pairs. Those pairs must remain `pass`; previously unsupported pairs
may improve from gap to pass. The original result row remains unchanged when a
required pass regresses, so failure evidence does not conceal the observed
classification.

## Inspected scope

Every changed line in `a8bcbe2..21c746f` was reviewed:

- `.github/workflows/ci.yml`
- `examples/support/conformance_catalog.rs`
- `examples/support/conformance_worker.rs`
- `examples/xml_conformance.rs`
- `tests/fixtures/xmlconf-20130923-required.tsv`
- `tests/xml_conformance_catalog_cli.rs`
- `tests/xml_conformance_cli.rs`
- `tooling/libxml2_xmlconf_catalog.c`

The C helper is limited to checksum-pinned, trusted XMLTS metadata. It requires
the pinned libxml2 compile-time and runtime version, applies DTD defaults,
resolves nested `xml:base`, rejects duplicate IDs, and fails closed on unknown
test types, malformed fields, unsafe URI forms, paths outside the suite root,
and configured field, path, and catalog-count bounds. Its emitted schema
contains all metadata used by the Rust policy. The nine archive path
corrections are exact ID-to-path exceptions for files absent at their recorded
locations and present at their corrected locations; both original and
effective paths remain visible.

The Rust catalog reader requires the exact header, field count, inventory
count, token vocabulary, unique IDs, UTF-8, LF line endings, and safe relative
paths. Its classification keeps XML-spec validity outcomes separate from
current implementation capability: unsupported entity processing, namespace-
disabled cases, parser limits, unsupported DTDs, and unsupported encodings are
gaps; XML 1.1, Namespaces 1.1, non-fifth-edition, and `TYPE=error` cases are
unscored. A conforming nonvalidating parser may accept a DTD-invalid document,
so `TYPE=invalid` requires acceptance. `TYPE=not-wf` requires rejection only
when no earlier capability rule prevents a meaningful comparison. Crossed
tests freeze those precedence rules for both accepted and rejected documents.

Each case runs in a bounded subprocess against reader, borrowed document,
owned document, push, and buffered-pull surfaces. The worker emits one strict,
typed row per surface and consumes the complete input. The parent rejects
stderr, nonzero exit, timeout, oversized output, missing, duplicate, reordered,
unknown, or extra rows, unknown error kinds, and impossible byte offsets. Input
paths are canonicalized under the canonical suite root, checked as regular
files before opening, read with a hard byte ceiling, and verified again through
the opened file handle. The parent always emits the complete five-row inventory
for a catalog case and exits nonzero for a mismatch, inconclusive result, or
required-pass regression.

The required-pass parser has its own versioned header, exact record and pair
counts, LF/UTF-8 rules, a shared input-size ceiling, unique known catalog IDs,
and exactly five binary surface bits with at least one required pass per row.
The committed inventory is sorted and was independently regenerated from the
frozen successful result table: its body matched byte-for-byte at 407 IDs and
1,871 pairs. Aggregate pass counts are not used as a substitute for identity.

CI pins and verifies the official archive and libxml2 source, compiles the C
helper with strict warnings, verifies its version, runs its independent
negative catalog tests, freezes the suite's complete file inventory before the
run, and compares it with the complete inventory afterward. At the executable
freeze it preserves the catalog, results, diagnostics, reviewed source hashes,
and both suite-inventory manifests as artifacts. The required-pass fixture is
included in the source hashes; local evidence separately retains the fixture
and runner/helper binary identities. A post-freeze CI-only supplement below
closes those artifact-retention differences.

## Contract alignment

| Requirement | Review result |
| --- | --- |
| Complete pinned catalog inventory | Met in code and focused helper evidence: exact count/header, duplicate rejection, complete type accounting, bounded metadata, and exact manifest reproduction. |
| Honest policy classification | Met for the bounded slice. Valid and invalid documents must be accepted; applicable not-well-formed documents must be rejected; capability gaps and out-of-scope metadata remain explicit. |
| All public parser surfaces | Met. Reader, borrowed DOM, owned DOM, push, and buffered pull are required in exact order for every case. |
| Fail-closed worker protocol | Met. Output size, time, exit status, stderr, row count/order/type, error vocabulary, and offsets are checked. |
| Supported-subset regression gate | Met. CI supplies a strict per-ID, per-surface required-pass baseline; any required pair that ceases to pass fails the run without rewriting its result row. |
| Bounded and stable inputs | Met. Catalog fields, paths, inventory, document size, child process output, and duration have fixed limits; suite identities are checked before and after CI execution. |
| Reproducible external oracle | Met at the code boundary. The helper requires pinned libxml2 2.15.4; CI separately requires the official archive checksum before extraction. Final run evidence remains a separate pending gate. |

## Deliberate mutation evidence

Each fault was introduced separately in the isolated review worktree, its
named test was run, and the changed source was restored before the next fault.

| Fault | Predicted detection | Observed result |
| --- | --- | --- |
| Treat a rejected `TYPE=invalid` document as a pass | `catalog::tests::classification_preserves_spec_and_capability_distinctions` | Failed: observed `pass` instead of the required `mismatch`. |
| Allow an external-entity `TYPE=not-wf` case to bypass the capability-gap rule | `catalog::tests::classification_preserves_spec_and_capability_distinctions` | Failed: observed `mismatch/unexpected-acceptance` instead of `gap/external-entity-processing`. |
| Accept trailing worker output after the five required rows | `tests::worker_protocol_is_complete_ordered_and_typed` | Failed on the appended `extra` row. |
| Disable the canonical suite-root containment check | `actual_command_rejects_cli_catalog_and_root_failures_without_dropping_rows` | Failed because the escaped symlink was accepted and the command succeeded. |
| Permit duplicate catalog IDs in the C helper | `pinned_catalog_cli_emits_complete_rows_and_rejects_bad_manifests` | Failed because the deliberately duplicated manifest was accepted. |
| Return success after a mismatch or inconclusive classification | `actual_command_preserves_full_inventory_exit_policy_and_worker_bounds` | Failed at the assertion requiring a nonzero exit status. |
| Disable the required-pass regression latch while preserving gap classification | `actual_command_preserves_full_inventory_exit_policy_and_worker_bounds` | Failed at the assertion requiring the selected DTD gap to make the command unsuccessful. |

These mutations establish sensitivity to the seven injected faults only. They
do not prove the absence of other faults.

## Verification evidence

The reviewer ran the following checks at the restored frozen tip:

- `cargo +stable test --offline --locked --example xml_conformance`: 12 passed.
- `cargo +stable test --offline --locked --test xml_conformance_cli`: two
  passed.
- The ignored `xml_conformance_catalog_cli` test with the locally compiled,
  pinned helper: one passed.
- `cargo +stable clippy --offline --locked --all-targets -- -D warnings`:
  passed.
- `cargo +stable fmt --all -- --check`: passed.
- `git diff --check a8bcbe2..21c746f`: passed.

The helper was independently rebuilt with C11, `-Wall -Wextra -Werror
-pedantic`, the pinned 2.15.4 headers, and the pinned static library. Its
version output was exactly `compiled=21504 runtime=21504`. Running it against
the official extracted catalog produced empty stderr and a 2,586-line manifest
byte-identical to `/private/tmp/yaxml-xmlconf-catalog-21504.tsv`, with SHA-256
`bb1245f6adf9296b000db1c12b03cb3da9c8d8474548f20edcc507b3cbf2da39`.

The `src` tree object is
`34f9acb22ae65b99fcea2cc1cc52e9ee6e036b3f` at both base and reviewed tip.
`Cargo.toml` and `Cargo.lock` are also byte-identical at both boundaries. The
reviewed tip's tree object is
`a53ad1d8faf341ee35628394c092bc82eeae5849`. The checked-in required-pass
manifest has SHA-256
`511ab5fc131106e193e7ee338f16562f9eb2c73d51ebc1cba12e916c14bff341`.

## Evidence limitations

This runner scores acceptance or rejection for the explicitly classified P0
slice. It does not yet compare canonical output, DTD validation diagnostics,
error locations, tree shape, entity expansion, or the full libxml2/XMLTS
behavior surface. Gap and unscored results are retained rather than converted
into passes. Differences between eager and streaming failure order may produce
different capability-gap totals without establishing a semantic acceptance
difference.

The final clean full-suite and official-suite artifacts must be reviewed
against this exact executable/source boundary before the milestone evidence is
closed. This report deliberately makes no claim about their in-progress row
counts, status distribution, diagnostics, or terminal result.

## Completed evidence supplement

This append-only supplement closes the external evidence gate at the exact
reviewed executable checkpoint
`21c746fd6e05281bda86764ee5e6dfebd2e12086`. It preserves the historical
pending statements above rather than rewriting the earlier review boundary.

The final root checks all exited successfully. The all-target run contained
470 passing tests and eight ignored tests across 37 test binaries; a separate
doctest run passed seven ordinary and four compile-fail examples. Strict
all-target Clippy and formatting checks passed. The retained logs are:

- `/private/tmp/yaxml-xmlconf-all-targets-21c746f.txt`
- `/private/tmp/yaxml-xmlconf-doctests-21c746f.txt`
- `/private/tmp/yaxml-xmlconf-clippy-21c746f.txt`
- `/private/tmp/yaxml-xmlconf-fmt-21c746f.txt`

The final pinned-suite command completed all 2,585 catalog cases and emitted
exactly 12,925 unique case/surface rows. There were zero mismatch and zero
inconclusive rows. Pass counts were 372 for reader, 372 for borrowed document,
407 for owned document, 360 for push, and 360 for buffered pull. The remaining
rows stay explicitly classified as capability gaps or unscored cases. In
particular, none of the 812 `TYPE=valid` cases per surface is currently scored
as a pass: 728 are capability gaps and 84 are unscored. The passing acceptance
cases are DTD-invalid documents that a nonvalidating parser must accept; the
remaining passes are applicable `TYPE=not-wf` rejections. This result does not
claim that the current P0 parser implements DTD processing, external entities,
all catalog encodings, namespace-disabled parsing, XML 1.1, historical XML
editions, or discretionary `TYPE=error` behavior.

The required-pass gate checked all 1,871 frozen case/surface pairs with zero
regressions. The raw result table has SHA-256
`c68982b4e33d2bb55a2dc59f1aa0fd30ac185cd61b3a97809667d8e96b67b99d`.
Its stderr contains only the expected per-surface/count and required-pass
summary; the wrapper's error stderr is empty.

An independent audit replayed every worker result and recomputed the catalog
policy for all rows. It completed 93,096 assertions with no discrepancy,
verified all and only the nine explicit path corrections, and independently
derived a required-pass body byte-identical to both the checked-in fixture and
the earlier `77af2c2` result. The audit records per-surface masks of reader 372,
document 372, owned 407, push 360, and buffered 360. Its report and retained
inputs are under:

- `/private/tmp/yaxml-xmlconf-evidence-21c746f/independent-audit/final-audit.txt`
- `/private/tmp/yaxml-xmlconf-evidence-21c746f/independent-audit/`

The fresh evidence records a clean start and end tree, 121 matching tracked
build-input identities, 3,386 matching suite-file identities with no header,
and identical start/end hashes for the Rust runner and C helper. The catalog
helper again reported compile-time and runtime libxml2 2.15.4, emitted empty
catalog stderr, and produced the same catalog SHA-256
`bb1245f6adf9296b000db1c12b03cb3da9c8d8474548f20edcc507b3cbf2da39`.
The complete evidence directory is
`/private/tmp/yaxml-xmlconf-evidence-21c746f`.

The review verdict remains **PASS** for this bounded acceptance/rejection
conformance runner and its regression gate. The broader limitations above
remain open; this evidence does not establish full XML, DTD, or libxml2
conformance.

## Post-freeze CI and documentation supplement

Commits `e4c1ee511b1b15b6ba7f6bf992722416637e775f` and
`86286497fd12dd3e5f4da6e02d3528f5aa8bac0d` change only CI artifact retention
and documentation after executable freeze `21c746f`. Production source,
Rust/C runner and helper inputs, tests, fixtures, Cargo files, fuzz harnesses,
and benchmarks remain byte-identical to the measured checkpoint.

The nine-line CI delta copies the exact required-pass fixture into the uploaded
artifact, records start and end SHA-256 identities for the release runner, C
catalog helper, and pinned static libxml2 archive, compares those manifests,
and uploads both. This closes the artifact-retention limitation recorded at the
original review boundary. The workflow YAML parses, and the extracted 45-line
catalog step passes `bash -n`. No executable test rerun is attributed to this
CI-only delta.

The new validation report, conformance contract, roadmap/security links, and
95-line machine-readable summary were reviewed against the raw final evidence.
The summary was independently regenerated byte-for-byte from all 12,925 result
rows and has the documented SHA-256
`32498e875ea9c3053a098198fa3c0c30ba561080b9a7896dc5672ce577105e25`.
The report's interface/type/status/reason totals reconcile to every raw row;
its 432 `OUTPUT` references resolve to regular extracted files and the release
contains no `OUTPUT3` references. It distinguishes the helper's libxml2 version
check from CI's archive checksum check and limits typed-error/source-offset
retention to rejected rows.

The final documentation preserves the bounded claim: complete catalog
inventory and acceptance classification with a supported-subset regression
gate. DTD semantics, external entities, canonical output, XML/Namespaces 1.1,
legacy encodings, broader compatibility, performance, and complete fuzz
coverage remain open. The review verdict remains **PASS**.
