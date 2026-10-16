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
run, and compares it with the complete inventory afterward. It preserves the
catalog, required-pass baseline, results, diagnostics, runner/helper
identities, and both suite-inventory manifests as artifacts.

## Contract alignment

| Requirement | Review result |
| --- | --- |
| Complete pinned catalog inventory | Met in code and focused helper evidence: exact count/header, duplicate rejection, complete type accounting, bounded metadata, and exact manifest reproduction. |
| Honest policy classification | Met for the bounded slice. Valid and invalid documents must be accepted; applicable not-well-formed documents must be rejected; capability gaps and out-of-scope metadata remain explicit. |
| All public parser surfaces | Met. Reader, borrowed DOM, owned DOM, push, and buffered pull are required in exact order for every case. |
| Fail-closed worker protocol | Met. Output size, time, exit status, stderr, row count/order/type, error vocabulary, and offsets are checked. |
| Supported-subset regression gate | Met. CI supplies a strict per-ID, per-surface required-pass baseline; any required pair that ceases to pass fails the run without rewriting its result row. |
| Bounded and stable inputs | Met. Catalog fields, paths, inventory, document size, child process output, and duration have fixed limits; suite identities are checked before and after CI execution. |
| Reproducible external oracle | Met at the code boundary. The C helper requires pinned libxml2 2.15.4 and the official archive checksum; final run evidence remains a separate pending gate. |

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
