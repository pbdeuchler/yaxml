# Construction fuzz-gap holistic review

**Plans:** `docs/plans/owned-create-slice.md` and
`docs/plans/owned-construction.md`

**Base:** `51e0aea6ed5e56eb4454139072474147fa71178b`

**Reviewed tip:** `011b0e2ca9a50c80addca4264f73d3e6592ea6db`

**Scope:** 15 commits; seven files; 2,629 insertions and three deletions

## Verdict

**PASS for code quality at the reviewed boundary.** No Critical, Important, or
Minor finding remains in the construction validation, resource-limit,
namespace, topology, dispatch, or corpus additions. Production source, Cargo
inputs, and dependencies are unchanged from the base.

Fresh source-complete fuzz coverage and a bounded feedback campaign are still
pending external evidence at this boundary. This report does not predict their
counts or claim progress toward the overall 100% fuzz-execution goal.

## Findings

### Critical

None.

### Important

None.

### Minor

None. An interim namespace review observed that successful append checks did
not compare every old link projection. Commits `c1d349d` and `011b0e2` close
that gap by anchoring the new topology to the returned handle and checking
document order, roots, declaration, children, first/last links, the changed
previous last child's next link, complete descendant lists, semantic fields,
and source provenance.

## Inspected scope

Every changed line in `51e0aea..011b0e2` was reviewed. The production crate is
unchanged. The diff adds three child models beneath the existing bounded
construction model, invokes construction for every accepted fuzz-driver input,
adds a dispatch test seam, and adds one construction-control corpus input.

The validation catalog contains 42 literal invalid/control pairs and executes
both creation and append paths, for 84 deterministic invocations. Expectations
are written before the public call. XML-character rejection precedes name
syntax rejection, non-ASCII locations use UTF-8 byte offsets, element semantic
locations retain the implicated component, and namespace/attribute item
semantic errors use item-only locations. Append failures compare a complete
public snapshot and retry the valid control with the same editor.

The limit model hand-counts zero, below, exact, and above boundaries. It covers
namespace scope steps, ordinary ancestor attributes, URI/prefix/expanded-name
comparisons, generated prefix length, generated item count, depth, explicit
items, persistent created-node and created-attribute ceilings, removal without
refund, and non-root component and aggregate string limits. Namespace-item
prefix and URI cases include a first-excess offset inside a multibyte scalar.
The generated-item precedence case uses two explicit attributes: automatic
`{urn:auto}a="1"`, followed by invalid unnamespaced `unbound:b="2"`.
With `max_attributes = 2`, planning the first attribute's `xmlns:ns1`
declaration must return global `LimitExceeded(Attributes)` before resolving the
second attribute. At three items, the invalid form reaches its exact
`AttributeItem { index: 1, component: None, byte_offset: None }` namespace
error, while the ordinary unprefixed `b="2"` control succeeds with exactly
three ordered attributes.

The namespace model has 17 fixed cases with input-derived valid local names.
Every parsed-parent case runs over UTF-8, UTF-16LE with BOM, and UTF-16BE with
BOM. The model checks implicit `xml`, inherited/default binding reuse, default
undeclaration, shadowing, deterministic generated prefixes, later exact-prefix
reservation, automatic reuse of a local explicit declaration, conflicts with
local and earlier generated bindings, duplicate expanded names across lexical
aliases, and equal locals in distinct namespaces. Parsed source buffers are
zeroed before mutation. Existing parsed nodes retain exact ranges; created
elements have no source provenance.

All selectors use fixed modulo catalogs. Derived namespace-model names consume
at most eight input bytes, parsed fixtures and item lists are constant-sized,
and each selected case performs bounded work. The driver now invokes this
bounded model before grammar and encoding dispatch for empty, malformed,
sparse-selector, UTF-16, and ordinary XML inputs. This makes the existing
`0x2a` and `0x2b` sparse construction paths reachable through the real replay
entry point.

## Contract and plan alignment

| Requirement | Review result |
| --- | --- |
| Independent success/error expectations | Met. Literal catalogs and hand-counted limits do not call the production prefix planner or validator to derive expected results. |
| Exact validation details | Met. Kind, item, component, and UTF-8 byte offset are asserted, with paired valid controls. |
| Resource and precedence boundaries | Met. Namespace work, generated names/items, shape, persistent quotas, supplied bytes, and component bytes include failure and successful controls. |
| Failure atomicity and recovery | Met. Failed appends compare snapshots plus serialization where applicable and perform a same-editor valid operation. Exhausted immutable creation ceilings use a non-creation recovery operation; a later raised-limit editor proves append recovery. |
| Namespace semantics | Met for the bounded catalog. Lexical and expanded names, ordered attributes/declarations, shadowing, aliases, and generated-prefix conflicts are explicit. |
| Stable topology and provenance | Met. The returned child is fresh, last under its parent, present at the exact preorder position, and reflected in old ancestors' descendant lists without changing unrelated public state. |
| Actual fuzz-driver reachability | Met. Construction runs once before all grammar/encoding returns; the test-only counter proves dispatch for representative inputs. |
| Bounded execution | Met. Input-dependent loops and allocation sizes are capped by existing driver limits or much smaller local constants. |

## Deliberate mutation evidence

Each fault was applied separately in an isolated worktree and restored. The
retained logs show the predicted public-model failure.

| Fault | Predicted and observed detection | Retained log |
| --- | --- | --- |
| Restore the former outer construction-dispatch gate | `construction_runs_before_grammar_and_encoding_dispatch_for_every_input` reports that empty input skipped construction. | `/private/tmp/yaxml-construction-limits-review-mutant-dispatch.log` |
| Permit the reserved XMLNS namespace URI in a declaration | The validation catalog rejects the accepted family 10/subtype 3 root construction. | `/private/tmp/yaxml-construction-validation-fuzz-mutant.log` |
| Remove a required namespace-work charge | `namespace_work_catalog_checks_below_exact_and_above` rejects the changed below/exact boundary. | `/private/tmp/yaxml-construction-limits-mutant-work.txt` |
| Stop publishing the persistent created-node counter | `successful_append_expenditure_persists_across_removal_and_reopen` observes that an exhausted second creation was accepted. | `/private/tmp/yaxml-construction-limits-review-mutant-persistence.log` |
| Remove only the incremental `planned_count > max_attributes` gate, retaining the final-count guard | `generated_bounds_check_name_item_count_and_error_precedence` receives the later item-1 `Namespace` error instead of global `LimitExceeded(Attributes)`. The precedence-case author commit was `f12395073b5df943ff20432bbe7664503c6a6b91`; its integrated commit is `17ee5ee`. | `/private/tmp/yaxml-construction-limits-precedence-mutant.log` |
| Omit subtree-size publication while retaining the new node/link records | The finite namespace catalog reports `constructed element is absent from document order`. | `/private/tmp/yaxml-construction-topology-mutant.txt` |

The source faults were restored before this review artifact was written. The
new models also contain negative oracle tests for a changed expanded namespace,
a shifted attribute quota offset, and a shifted namespace-item quota offset.

## Verification evidence

The reviewer ran these focused checks at `011b0e2`:

- `finite_namespace_catalog_matches_literal_expectations_in_utf8_and_utf16`:
  passed.
- `generated_bounds_check_name_item_count_and_error_precedence`: passed.
- `git diff --check 51e0aea..011b0e2`: passed.

Earlier independent focused reviews passed the complete 42-case validation
catalog, arbitrary/short selector cases, the seven-test limit catalog, strict
Clippy for the fuzz-smoke target, and formatting. The final root all-target,
doctest, strict Clippy, and formatting runs were still in progress when this
report was written and are not claimed here.

The `src` tree object is
`34f9acb22ae65b99fcea2cc1cc52e9ee6e036b3f` at both base and reviewed tip.
`Cargo.toml` and `Cargo.lock` blobs also match exactly. The reviewed fuzz/test
blobs are:

- `fuzz/construction.rs`: `9e5e18af953762120645dfcaeb704ce671b2b376`
- `fuzz/construction_limits.rs`: `d7faaca4dbf80463e18fc1cdf7518773b4ff6827`
- `fuzz/construction_namespaces.rs`: `f8c953a483dc40bdab1f2b3effb3814c3bf1d9c1`
- `fuzz/construction_validation.rs`: `2054a51fe45af0888267dd3e1348198251f2a4e6`
- `fuzz/driver.rs`: `213373e29050333e7aaf7e5262a8fea8d8a2eb0e`
- `tests/fuzz_smoke.rs`: `8949d4e63c8fb6a869558a57e4cd8f225e9c35b1`

## Evidence limitations

These deterministic catalogs are bounded examples, not proof over all XML
namespaces, quota combinations, allocation failures, or operation sequences.
Mutation results establish sensitivity to the injected faults only. No new
production behavior is introduced by this diff. Fresh source-complete fuzz
coverage must retain exact source, harness, compiler, LLVM-tool, profile, and
exclusion identities. A bounded campaign must separately retain its corpus,
journal, first-failure evidence, and target/tool identities. Until those
artifacts are reviewed, this report makes no new coverage percentage or
sustained-security claim.
