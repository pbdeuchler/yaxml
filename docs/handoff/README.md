# yaxml project handoff

Snapshot date: 2026-10-18, America/Denver.

The project is building a safe, pure-Rust replacement for libxml2. The core
forbids unsafe code and keeps `memchr` as its only dependency. Compatibility,
security, coverage and performance are measured separately; none of the
documents in this handoff claim full libxml2 compatibility, 100% coverage or
across-the-board performance superiority.

The immediate workstream is the private DTD Reader fuzz campaign. Its Cargo
process/profile integration is implemented at commit
`da0fd3d5599b133129243b997667ff8a5181da61`, and fresh real-tool evidence has
been sealed. The candidate is **not yet accepted or integrated**. The two final
end-to-end reviews were started after evidence regeneration, but the review
agents exhausted their usage allowance before producing reports. Do not
integrate the remaining series until both reviews are rerun and pass.

Read these files in order:

1. [Project status](project-status-2026-10-18.md) summarizes the program,
   delivered capabilities, open goals and major risks.
2. [DTD process-integration status](dtd-reader-fuzz-process-integration-2026-10-18.md)
   records the exact branches, commits, review results and evidence identities
   for the active workstream.
3. [Resume runbook](resume-runbook-2026-10-18.md) gives the safe sequence for
   finishing review, integrating the candidate and starting the next slice.

Canonical status at this snapshot:

| Surface | Identity | State |
| --- | --- | --- |
| Local `main` | `fc6c3c7322083fde84cb498e02fef307799850c4` | Documentation through the accepted source-inventory handoff |
| Private DTD integration | `693389925453505ad5c649c66dfc1e76a654c961` | Source-inventory handoff integrated and accepted |
| Process candidate | `da0fd3d5599b133129243b997667ff8a5181da61` | Implemented, repaired and evidenced; final full-series review pending |
| Final candidate tree | `db9b78ad1cefc4d59c4d113efeaea576d00bb0d6` | Must remain exact through final review |
| Final evidence archive | `6d5ee8905bb196c90e5dbd3ab6e3d73ffa1388cd80928fe836d906758aadb40f` | Verified, stored under `/private/tmp`; preserve before cleanup or reboot |

The visible workspace path did not contain a `.git` entry when this handoff was
written. Use the registered worktrees under `/private/tmp` for Git operations;
the runbook names the authoritative paths.

## Superseded (2026-10-18 evening)

This handoff is superseded by [the working process](../process.md). The
process candidate (`da0fd3d`) was not integrated. Its Cargo/libtest campaign
tooling, along with the earlier campaign harness, was removed in favor of
cargo-fuzz. The private DTD stack was merged into main ahead of public
activation.
