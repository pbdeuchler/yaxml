# Resume runbook

This runbook completes the current process-integration gate without weakening
the established review or evidence rules.

## 1. Re-establish exact state

Use the registered Git worktrees. The visible workspace path lacked `.git` at
the handoff snapshot and should not be used for branch operations.

```sh
git --git-dir=/private/tmp/yaxml-20261015.git worktree list --porcelain

git -C /private/tmp/yaxml-process-integration status --short
git -C /private/tmp/yaxml-process-integration rev-parse HEAD
git -C /private/tmp/yaxml-process-integration rev-parse 'HEAD^{tree}'

git -C /private/tmp/yaxml-dtd-integration status --short
git -C /private/tmp/yaxml-dtd-integration rev-parse HEAD
git -C /private/tmp/yaxml-dtd-integration rev-parse 'HEAD^{tree}'
```

Required identities:

- process candidate head:
  `da0fd3d5599b133129243b997667ff8a5181da61`;
- process candidate tree:
  `db9b78ad1cefc4d59c4d113efeaea576d00bb0d6`;
- DTD integration head:
  `693389925453505ad5c649c66dfc1e76a654c961`; and
- DTD integration tree:
  `45eb8f856bde251eeda01c6f082a423b5d8f97a5`.

Both worktrees must be clean. Investigate any difference before proceeding.

Verify the evidence archive before using it:

```sh
shasum -a 256 /private/tmp/yaxml-process-evidence-da0fd3d-r11.tar.gz
shasum -a 256 /private/tmp/yaxml-process-evidence-da0fd3d-r11.tar.gz.sha256
shasum -a 256 -c /private/tmp/yaxml-process-evidence-da0fd3d-r11.tar.gz.sha256
shasum -a 256 /private/tmp/yaxml-process-integration-final-evidence-generation-report.md
```

Expected SHA-256 values are, in the same order:

```text
6d5ee8905bb196c90e5dbd3ab6e3d73ffa1388cd80928fe836d906758aadb40f
252e4eeea1c797f06e062e0fa430fb001849c37fc78519873c63df2dbd393359
OK
978cc315c71c0de3fdef841b5328663c39bb81930d3c69759a71631631590cab
```

If the archive is missing or any identity differs, regenerate it from the exact
candidate before review. Do not reuse the older `r10` archive.

## 2. Rerun the two final reviews

Run two independent reviews against the complete series
`a8f8c501..da0fd3d`, final tree
`db9b78ad1cefc4d59c4d113efeaea576d00bb0d6`, and the exact `r11` archive.

The conformance review must independently verify:

- every requirement in the accepted process matrix;
- exact Cargo/libtest command, environment and protocol behavior;
- source-inventory continuity through preparation, execution, LLVM tools,
  profile commit, audit and report publication;
- Master31, Stable21, all eight fields for every intersection and aggregates;
- every ledger frame and final count/bytes/digest;
- archive self-containment, root and nested checksums, source copies and
  live/extracted equality;
- Unix identity behavior and Windows warning-denied cross-check/fail-closed
  campaign boundary; and
- complete test and tool receipts.

The holistic review must inspect every changed line in the five remaining
commits for correctness, security, failure ordering, boundedness, portability,
maintainability and test quality. It must review the archive independently and
exercise crucial discriminators. Perform mutations only in a disposable copy
under `/private/tmp`; never mutate `/private/tmp/yaxml-process-integration`
while another reviewer is reading it.

Each reviewer must write a Markdown report, state severity counts and PASS or
FAIL, and publish the report SHA-256. Any finding blocks integration. Do not
reuse the repair-only PASS reports as final verdicts.

## 3. Integrate only after two PASS verdicts

Cherry-pick the remaining series onto the private DTD integration branch in
this exact order:

```sh
git -C /private/tmp/yaxml-dtd-integration cherry-pick 4e09d54
git -C /private/tmp/yaxml-dtd-integration cherry-pick 2c316f2
git -C /private/tmp/yaxml-dtd-integration cherry-pick 73363e2
git -C /private/tmp/yaxml-dtd-integration cherry-pick a31117b
git -C /private/tmp/yaxml-dtd-integration cherry-pick da0fd3d
```

Because the candidate base tree and integration head tree are identical, the
post-cherry-pick integration tree should be exactly:

```text
db9b78ad1cefc4d59c4d113efeaea576d00bb0d6
```

Stop and investigate if it differs.

## 4. Validate the integrated tree

Use isolated target directories. The two socket tests are filtered only because
the managed sandbox returns `EPERM` when creating Unix-domain sockets; retain
that fact in the integration report.

```sh
cd /private/tmp/yaxml-dtd-integration

CARGO_TARGET_DIR=/private/tmp/yaxml-process-final-integration-tests \
  cargo test --all-targets -- --test-threads=1 \
  --skip cargo_artifact::tests::rejects_unix_domain_socket_as_final_executable \
  --skip cargo_artifact::tests::selected_recheck_rejects_unix_socket_as_initial_nonregular_file

CARGO_TARGET_DIR=/private/tmp/yaxml-process-final-integration-tests \
  cargo test --release --all-targets -- --test-threads=1 \
  --skip cargo_artifact::tests::rejects_unix_domain_socket_as_final_executable \
  --skip cargo_artifact::tests::selected_recheck_rejects_unix_socket_as_initial_nonregular_file

CARGO_TARGET_DIR=/private/tmp/yaxml-process-final-integration-tests \
  cargo clippy --all-targets --all-features -- -D warnings

cargo fmt --all -- --check
git diff --check

RUSTFLAGS=-Dwarnings rustup run nightly-2026-06-06 \
  cargo check --example fuzz_campaign --target x86_64-pc-windows-msvc
```

Also rerun the ignored pinned real fixture with the exact Cargo, rustc,
`llvm-profdata`, `llvm-cov` and LLVM 22.1.6 environment recorded in
`/private/tmp/yaxml-process-integration-final-evidence-generation-report.md`.
Run the FrozenProfile and FrozenProfileLedger focused suites. Confirm the
integration worktree is clean after validation.

Write a final integration report with the new cherry-pick commit IDs, exact
tree, commands, results, reviewer report hashes and archive hash. Add a concise
repository review document beside the existing DTD process review files.

## 5. Continue the DTD fuzz burndown

After process integration is accepted, start stage 4 of
[the DTD Reader fuzz plan](../archive/plans/dtd-reader-fuzz.md):

1. add a bounded streaming DTD Session driver covering UTF-8/UTF-16 transport,
   chunk boundaries, lending/callback behavior and generated scheduling;
2. add a bounded DTD Buffered Reader driver covering `fill_buf`/`consume`, held
   and generated draining, segmentation and sticky I/O failures;
3. add a bounded private DTD Writer semantic-event driver;
4. freeze the exact focused-guard test list for allocation, unwind, failpoint
   and invariant-only branches;
5. give every entry a complete source manifest and a separate raw profile; and
6. allow profile merging only for byte-identical libtest executables and object
   mappings.

Stage 5 then freezes the pinned LLVM denominators and closes every remaining
stable-entry and combined private-DTD gap by exact source location. Public DTD
activation, broad libxml2 compatibility and performance claims remain separate
milestones.

## Operational cautions

- `/private/tmp` may be cleaned. Preserve the final archive and reports before
  relying on them across a reboot or long pause.
- The pinned toolchain is `nightly-2026-06-06` (`rustc`/Cargo 1.98 nightly,
  LLVM 22.1.6). Local stable Rust was 1.95 at the snapshot.
- The Windows target `x86_64-pc-windows-msvc` was installed for the pinned
  toolchain.
- The campaign example intentionally fails closed on non-Unix platforms because
  stable safe persistent Windows file identity is unavailable within the
  no-unsafe/one-dependency constraints.
- Do not let reviewers mutate the shared candidate worktree concurrently. One
  prior transient test failure was caused by two deliberate mutations
  overlapping; both were restored byte-exact and the clean rerun passed.
- All review agents from the interrupted final pass ended with usage-limit
  errors. No agent work was still in flight when this handoff was written.
