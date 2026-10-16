# Pinned XML conformance inventory validation

The executable freeze is `21c746fd6e05281bda86764ee5e6dfebd2e12086`, based
on main `a8bcbe26748c7baa876ab5f4cf8ce887834e92dc`. The implementation adds
an external suite runner, catalog metadata helper, regression baseline and
tests. Production Rust, Cargo inputs and fuzz harnesses are byte-identical to
the base; the `src` tree is `34f9acb22ae65b99fcea2cc1cc52e9ee6e036b3f`.
Later changes retain additional CI artifacts and document/review the evidence;
the measured Rust/C executables and pass baseline remain unchanged.

## Pinned inputs and independent expectations

The [official W3C release](https://www.w3.org/XML/Test/xmlts20130923.tar.gz)
has observed SHA-256
`9b61db9f5dbffa545f4b8d78422167083a8568c59bd1129f94138f936cf6fc1f`.
The complete root catalog has **2,585** unique cases: 812 `valid`, 242 `invalid`,
1,498 `not-wf` and 33 discretionary `error`. It includes XML 1.0/1.1 and
Namespaces 1.0/1.1; these are not replaced with a hand-picked subcatalog list.
Fixtures remain in the external archive rather than being vendored into yaxml.

The metadata helper is an external libxml2 **2.15.4** executable, with compiled
and runtime version `21504`. It uses the reference library's declaration
defaults, entity expansion and XML Base resolution to emit metadata, never to
parse testcase documents for yaxml. Its static reference library has SHA-256
`b48612c66e7d4d7fae1ceb7db9a0d3e288085bc91b560f037862607eccac1ab0`.
The generated 2,585-row catalog has SHA-256
`bb1245f6adf9296b000db1c12b03cb3da9c8d8474548f20edcc507b3cbf2da39`.
All 12 fields per row and the complete ordered ID inventory were independently
checked against separately expanded catalog XML, parsed by Ruby REXML with
independent inherited-base resolution. Helper diagnostics are empty.

The [runner contract](conformance.md#complete-external-catalog) defines scoring,
bounds and the nine explicit archive path corrections. The suite DTD requires
nonvalidating parsers to accept `invalid` documents too. A capability gap is
never a successful negative result; an unsupported first error does not verify
the remaining grammar. The optional `OUTPUT` files describe an additional
canonical-output contract: all 432 references exist, and none is compared by
this acceptance runner. There are no `OUTPUT3` references in this release.

## Complete run

Fresh release execution produced exactly **12,925 rows**, five per case, with
zero mismatches, inconclusive outcomes or required-pass regressions. Its table
has SHA-256
`c68982b4e33d2bb55a2dc59f1aa0fd30ac185cd61b3a97809667d8e96b67b99d`.
The summary is expected nonempty stderr, with SHA-256
`5ad63ce5fcb401942f27ccd5b2b8754e3d85f67c39099cb89f2ff93b39d630c2`.

| Interface | Pass | Capability gap | Unscored | Mismatch | Inconclusive |
| --- | ---: | ---: | ---: | ---: | ---: |
| Reader | 372 | 1,602 | 611 | 0 | 0 |
| Borrowing DOM | 372 | 1,602 | 611 | 0 | 0 |
| Owned DOM | 407 | 1,567 | 611 | 0 | 0 |
| Push | 360 | 1,614 | 611 | 0 | 0 |
| Buffered pull | 360 | 1,614 | 611 | 0 | 0 |

These are **not full-suite pass rates**. None of the 812 `TYPE=valid` cases is
currently scored as passing: 728 are gaps and 84 are outside the selected
version/profile scope. DTD support is the main missing capability. The accepted
passing cases are `TYPE=invalid`, whose well-formedness requires acceptance:
71 on borrowing interfaces and 73 on auto-decoding interfaces. Passing negative
cases number 301 on borrowing interfaces, 334 on owned DOM and 287 on streaming
interfaces. Eager whole-input validation can find an illegal character where
streaming stops at an earlier unsupported DTD. Different gap totals therefore
do not establish different accepted languages.

| Capability gap | Reader / borrowing DOM | Owned DOM | Push / buffered pull |
| --- | ---: | ---: | ---: |
| DTD processing | 1,306 | 1,309 | 1,356 |
| External entity requirements | 247 | 247 | 247 |
| Encoding support | 40 | 2 | 2 |
| Namespace-disabled profile | 9 | 9 | 9 |

Each interface's 611 unscored rows comprise 274 unsupported recommendation/
version rows, 310 historical-edition rows and 27 discretionary-error rows,
using the documented first applicable classification. Every rejected row retains its
typed error and source-byte offset; the suite does not specify yaxml's exact
diagnostic categories or first-error precedence.

The [machine-readable counts](validation/xmlconf-20130923-21c746f.tsv) group
every row by interface, official type, status and reason. Its SHA-256 is
`32498e875ea9c3053a098198fa3c0c30ba561080b9a7896dc5672ce577105e25`.

## Regression gate and verification

The checked-in required-pass inventory has **407 IDs and 1,871 pairs**, with
SHA-256 `511ab5fc131106e193e7ee338f16562f9eb2c73d51ebc1cba12e916c14bff341`.
Independent derivations from the earlier `77af2c2` run and the final run match
it exactly. CI requires every listed pair to remain `pass`; a result changing
to unsupported or resource-limited is a failing regression, even though its
gap row remains honest. This protects identities rather than aggregate counts
and permits newly supported pairs to improve.

At the freeze, all-target tests passed **470 tests**, with eight ignored
external/timing tests; all five custom benchmark smoke executables completed.
All **11 doctests**, strict all-target Clippy and formatting passed. New tests
comprise 12 runner/catalog/worker unit tests, two actual-command tests, and one
ignored external-helper command test. That helper test was explicitly run
with the pinned executable and passed. It is wired into external-oracle CI.
The workflow YAML parses and its extracted shell passes `bash -n`; remote CI
was not executed in this local validation session.

The [holistic review](review-xml-conformance.md) records no remaining findings.
Restored faults in type classification, entity-gap precedence, worker trailing
output, root containment, duplicate IDs, mismatch exit status and required-pass
exit status were detected. The actual CLI tests verify complete row inventories,
error/gap distinctions, malformed metadata, missing/oversized/special inputs,
canonical symlink containment, and worker EOF/byte ceilings. A required DTD gap
must retain its gap row and still exit unsuccessfully.

The independent final audit checked all 121 recorded tracked build inputs
against committed blobs, all **3,386** extracted regular-file identities before
and after, and runner/helper binary identities. It independently replayed every
case through the release worker and reproduced the full result table and
classification. It verified all nine path corrections and their 45 result
rows. Audit diagnostics are empty. This does not imply independent semantic
tree or canonical-output comparison.

## Reproduction and limits

Local execution used rustc **1.95.0**, LLVM 22.1.2, release thin LTO and one
codegen unit on `aarch64-apple-darwin`. C compilation used C11 with
`-Wall -Wextra -Werror -pedantic -Wconversion -Wshadow`. The exact build/run
script is `/private/tmp/yaxml-xmlconf-final-baseline.sh`; evidence is
`/private/tmp/yaxml-xmlconf-evidence-21c746f`, including source and suite
manifests, compiler and reference identities, binaries, catalog, full results
and hashes. Independent scripts, replay and notes are in its
`independent-audit` directory. Root logs are
`/private/tmp/yaxml-xmlconf-{all-targets,doctests,clippy,fmt}-21c746f.txt`.

This milestone implements acceptance inventory and a regression gate. DTD
declaration effects, entity-created markup, external resolution, DTD validity,
canonical output, XML/Namespaces 1.1 and legacy encodings remain open. It is
not a performance comparison or a sustained fuzz campaign. The production
source and fuzz harness match the [previous fuzz milestone](validation-construction-fuzz-gaps.md)
exactly; its measured 93.80% region and 94.74% branch coverage remains the
separate fuzz evidence. These conformance results are not merged into or
relabeled as fuzz coverage. The full compatibility, security and performance
objectives remain active.
