# Private DTD Reader Cargo artifact-resolver review

Plan: [private DTD Reader fuzzing](plans/dtd-reader-fuzz.md).
Integrated commit: `798992008c3619bb0ae0e4520ee134baefca7a72`.
Tree: `1382568b126c174e4392e655cd71317a26c8b6d4`.
Base: `8e29a9bea2488348d61dbf0d483ee1fe0c5ca2a4`.

Verdict: PASS, with no open Critical, Important or Minor findings for the
filesystem resolver slice. Cargo process, CLI and campaign execution wiring
remain separate gates.

## Resolver boundary

The resolver establishes canonical source and target roots, rejects lexical
aliases before lookup, walks every path component with no-follow metadata, and
requires an exact component-aware descendant of the target directory. It
rejects final and intermediate symlinks, missing and wrongly typed components,
non-UTF-8 or control-bearing paths, and source metadata that differs from the
selected Cargo record.

The selected executable is opened once. Its stamp and FNV-plus-length content
identity come from that inspected handle, followed by a second complete path
walk, canonical equality check and stamp comparison. The direct-target campaign
uses the same shared identity implementation with its established behavior
preserved. Hard links remain accepted.

Independent review found and repaired three issues before integration: the
original post-capture symlink fixtures did not specifically detect deletion of
the second walk; exact canonical equality occurred after stamp/open; and the
frozen platform fixture set lacked a Unix-domain socket and Windows symlink
cases. The final sentinels detect those regressions as well as string-prefix
containment, canonicalization before walking, metadata-only identity and lost
lexical-dot rejection.

## Verification and limits

The integrated example passes 79 tests in debug and release; its focused
artifact-resolver subset passes 57 tests in each profile. All example binaries
pass in both profiles. Strict test/example Clippy, formatting and diff checks
pass. The Unix-domain socket fixture passed outside the managed macOS sandbox,
which denies socket creation. Windows test code cross-checks compile, but its
symlink behavior was not executed on this host and privilege denial is reported
as an explicit fixture skip.

The final conformance review passed with report SHA-256
`fc07090c9250d512d77d1a0f66a060285935837e76cc20cbcda289ee7b428b51`.
The final holistic review passed with report SHA-256
`fa93b7d641266cd715525b49d0265877d711692ae4aa7a042b160707fff6a63c`.

Portable `std::process::Command` still launches by path rather than by the
verified open handle. Replacement and restoration between the final check and
process image acquisition therefore remains a documented race. Portable `std`
also does not expose a uniform device/inode identity or every Windows reparse
point property. The resolver does not claim cryptographic provenance: FNV is an
accidental-change identity and filesystem timestamps have platform-dependent
resolution.
