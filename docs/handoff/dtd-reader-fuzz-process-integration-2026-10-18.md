# DTD Reader fuzz process-integration status

## Decision state

The Cargo process/profile integration candidate is code-complete and has fresh
real-tool evidence, but it is **not accepted or integrated**. Repair-specific
conformance and holistic reviews pass. The final reviews covering the complete
series and regenerated evidence archive were launched, then all available
review agents exhausted their usage allowance before returning reports.

The next owner must rerun those two final reviews. Do not infer a PASS from the
repair reviews or the evidence-generation report.

## Branch and commit topology

The Git directory is `/private/tmp/yaxml-20261015.git`.

| Role | Worktree | Branch | Head | Tree/state |
| --- | --- | --- | --- | --- |
| Local documentation main | `/private/tmp/yaxml-20261015.git` | `main` | `fc6c3c7322083fde84cb498e02fef307799850c4` | Includes review docs through the accepted inventory handoff |
| Private DTD integration | `/private/tmp/yaxml-dtd-integration` | `integrate/dtd-prototype` | `693389925453505ad5c649c66dfc1e76a654c961` | Tree `45eb8f856bde251eeda01c6f082a423b5d8f97a5`; clean at the last check |
| Full process candidate | `/private/tmp/yaxml-process-integration` | `implement/process-integration` | `da0fd3d5599b133129243b997667ff8a5181da61` | Tree `db9b78ad1cefc4d59c4d113efeaea576d00bb0d6`; clean at the last check |

The accepted source-inventory handoff was authored as `b10bf544`, `465b9e8`
and `a8f8c501` on the candidate line. It was integrated under new commit IDs
`bdc2741`, `4e6a86c` and `6933899`. Both heads have the same base tree
`45eb8f856bde251eeda01c6f082a423b5d8f97a5`.

The remaining candidate commits, in cherry-pick order, are:

1. `4e09d54` — adopt the Cargo target and preparation owner;
2. `2c316f2` — execute the exact Cargo/libtest target through frozen profiles;
3. `73363e2` — Master31/Stable21 audit, profile commit and ledger;
4. `a31117b` — final report and evidence lifecycle; and
5. `da0fd3d` — fail closed without safe portable file identity and add the
   post-target drift discriminator.

No commit in this remaining list has been cherry-picked to
`integrate/dtd-prototype`.

## Accepted design and inventory gates

The accepted process matrix is summarized in
[the process-matrix review](../review-dtd-reader-fuzz-process-matrix.md).

| Evidence | SHA-256 |
| --- | --- |
| Accepted matrix | `45d3871d5db888660beac14d5f8b55f2df29b44fa67bf918fe0f0b0851ebbcd0` |
| Matrix repair report | `885a0e20c28dc9e9d529d945ba02b4c88d3610c990fbdf32c0234942f3d3fcc0` |
| Final matrix conformance review | `8c9d5750ec08286ca360f0882422fb59475d55250f54f452ded10d6a348ec538` |
| Final matrix holistic review | `dad8a0abfc1eae3e42aa1551e3345e8267be290828dfe0f3c20c7e4cab5993f2` |

The source-inventory handoff is already integrated and accepted. Its review is
[recorded separately](../review-dtd-reader-fuzz-process-inventory-handoff.md).

| Evidence | SHA-256 |
| --- | --- |
| Final inventory conformance review | `336261f6f45e2cb4b2801bd5ac6b83063cc6d614581b92a66382af6757675d6d` |
| Final inventory holistic review | `2a85d97ffbbd23df18fcfc852d0e6bfa63c75920efa7a73eff3e97b2e29bf304` |
| Inventory integration report | `da32cf4bfac4e5679940d6110b7b125a9bcd081aab4089ee981b0957d70c22f5` |

## Full-series review history

The first final review covered `a31117b` / tree
`ba4eb57295ee48eac0b098ef057e7549d8969259` and correctly failed:

- the Windows target used unstable `MetadataExt::volume_serial_number` and
  `file_index`, and a non-Unix ledger branch had an incompatible placeholder
  key; and
- deleting the post-target/pre-protocol source gate did not make the claimed
  regression test fail.

| Failed review | Result | SHA-256 |
| --- | --- | --- |
| Final conformance review at `a31117b` | FAIL: one High | `16092e62dc53ca5d9f5092c94c5dffdc11af602bc4632a77c4f2ce0b16f8807c` |
| Final holistic review at `a31117b` | FAIL: two Important | `c086b61c68f50d885e037595189ba750783b678762ab6efa9e622944293c8575` |

The corresponding temporary reports are
`/private/tmp/yaxml-dtd-reader-step3-process-integration-final-conformance-review.md`
and
`/private/tmp/yaxml-dtd-reader-step3-process-integration-final-holistic-review.md`.

Commit `da0fd3d` repaired both findings. Unix retains `(dev, ino)` identity.
Non-Unix builds use no timestamp or unit placeholder as evidence; the campaign
returns a typed unsupported-platform error after `--help` handling and before
`Campaign::new`, filesystem setup or process launch. A test checkpoint now
introduces source drift after a successful target process and before the
retained source gate. The test requires a target/source failure, protocol
`NotRun` and only the target process receipt. Removing the production gate
turns the test red; byte-exact restoration turns it green.

| Repair evidence | Result | SHA-256 |
| --- | --- | --- |
| Repair implementation report | PASS | `dca0cb602612f8d1e8d956eaf46b1a3093501f0e957c32e3603d131109831ab9` |
| Repair conformance rereview | PASS, no findings | `cd82256ef6a4b8cf64e385ee6ee719a1aeef054e5fd9adefeab293f8ef88958a` |
| Repair holistic rereview | PASS, no findings | `1a9e4eda7788efa9e5917bc3d02668d715242f84e2e6e53fd064ff5123a8ad84` |

The repair reports are, respectively,
`/private/tmp/yaxml-process-integration-cross-platform-repair-report.md`,
`/private/tmp/yaxml-process-integration-cross-platform-repair-conformance-review.md`
and
`/private/tmp/yaxml-dtd-reader-step3-process-integration-repair-holistic-rereview.md`.

These are repair-scoped reviews. They do not replace the missing final reviews
of the complete series plus the regenerated archive.

## Fresh final evidence

The implementation agent regenerated evidence after the repair from exact
commit `da0fd3d5599b133129243b997667ff8a5181da61` and tree
`db9b78ad1cefc4d59c4d113efeaea576d00bb0d6`.

| Artifact | Path | SHA-256 |
| --- | --- | --- |
| Immutable archive | `/private/tmp/yaxml-process-evidence-da0fd3d-r11.tar.gz` | `6d5ee8905bb196c90e5dbd3ab6e3d73ffa1388cd80928fe836d906758aadb40f` |
| Archive sidecar | `/private/tmp/yaxml-process-evidence-da0fd3d-r11.tar.gz.sha256` | `252e4eeea1c797f06e062e0fa430fb001849c37fc78519873c63df2dbd393359` |
| Evidence-generation report | `/private/tmp/yaxml-process-integration-final-evidence-generation-report.md` | `978cc315c71c0de3fdef841b5328663c39bb81930d3c69759a71631631590cab` |

Archive closure at generation time:

- archive size: 19,513,660 bytes;
- 211 live files and 211 extracted files;
- 210 root `SHA256SUMS` entries, all valid in both trees;
- nested mutation checksums valid;
- sorted relative file-list diff: empty;
- independent all-file hash diff: empty;
- packaged `sources/src` versus final candidate `src`: recursive diff empty;
- archive sidecar written only after final archive bytes and verified with
  `shasum -a 256 -c`; and
- live and extracted aggregate checksum files byte-identical.

The actual campaign accepted its versioned protocol, Master31 and Stable21;
committed the candidate and current-candidate audit; and terminated at the
one-case limit. The ledger contains 18 records, 4,809 serialized bytes and final
rolling digest `aaa1e249f51b8308:4505`. The independent literal verifier passed
Master31 from the literal Master32 table, Stable21, all eight integer fields for
all 21 intersections, aggregate tuples, all 18 ledger frames and all frozen
source/tool/artifact/profile identities.

Validation recorded in the archive:

- full debug and release all-target commands passed with the two known Unix
  socket fixtures filtered because the managed sandbox rejects socket creation;
- the `fuzz_campaign` example reported 236 passed, zero failed and one ignored
  in both debug and release;
- the ignored pinned real Cargo/libtest fixture passed;
- FrozenProfile and FrozenProfileLedger each passed 9/9 tests;
- all-target/all-feature Clippy with warnings denied, rustfmt and diff checks
  passed; and
- the pinned Windows campaign check passed with `RUSTFLAGS=-Dwarnings`.

## Current blocker

Fresh final conformance and holistic reviews were requested against the exact
`r11` archive. The conformance reviewer, its evidence subreviewer and the
holistic reviewer all stopped with the same agent usage-limit error before
returning final reports. There is no open code finding from those attempts, but
there is also no valid PASS verdict. Rerun both reviews from scratch when review
capacity is available.
