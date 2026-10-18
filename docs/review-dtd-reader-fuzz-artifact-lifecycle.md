# Private DTD Reader selected-artifact lifecycle review

Plan: [private DTD Reader fuzzing](plans/dtd-reader-fuzz.md).
Integrated commit: `e3f7323129a4f713449f6cb249ed9c7fb827b500`.
Tree: `ad4ac76831c1a361a5f18723dd09707fcc043496`.
Base: `798992008c3619bb0ae0e4520ee134baefca7a72`.

Verdict: PASS, with no open Critical, Important or Minor findings for the
selected-artifact lifecycle stage. Cargo preparation and campaign wiring remain
separate gates.

## Lifecycle boundary

The resolver now exposes example-internal Metadata and Content checks against
the original selected artifact. Both modes revalidate trusted roots, lexical
policy, strict target containment, every executable path component, canonical
identity and the frozen file stamp. They never create a replacement selection
or advance the baseline.

Content checks open the selected path once and hash that inspected handle
through the shared 64 KiB FNV engine. The read stops at the frozen length plus
one growth byte, so a moving EOF cannot make the check unbounded. Pre-read and
post-read handle metadata plus a repeated path walk detect ordinary replacement,
growth, truncation and type drift. Stable unequal bytes receive a distinct
content-identity error.

Independent review found that self-referential symlink replacement after the
initial walk leaked the operating system's loop error. The repair re-inspects
the frozen path when post-walk canonicalization or open fails: proven path or
type drift becomes `ChangedSinceSelection`, while a stable path retains its
original I/O error. The reinspection is bounded, nonrecursive and does not hash
or rebaseline.

## Verification and limits

The integrated example passes 114 tests in debug and release. Independent
conformance caught 12 deliberate lifecycle regressions, passed nine standalone
fixtures in debug and optimized modes, and completed 1,341 all-target tests plus
13 doctests. Strict Clippy, formatting and diff checks pass.

The final conformance review passed with report SHA-256
`fb36d9c96bba01fb451af298f6e72101cc171a14421c1c4a7fc721dbff522f80`.
The final holistic review passed with report SHA-256
`d73d7bf1a4417bcdc4c91792eae3c1f042907a200db6c929f8b9354eb7afdfa3`.

Metadata checks deliberately cannot detect equal-length replacement with the
same observable timestamp. Content checks remain noncryptographic and retain
the portable check-to-launch race. Unix socket fixtures passed outside the
managed sandbox; Windows symlink behavior remains platform-unexecuted here.
