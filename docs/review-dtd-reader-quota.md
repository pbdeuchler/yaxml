# Private stable-input DTD Reader quota review

Reviewed candidate: `cc5a70934d6eecdb3c360fa13d3aaf17b1378037`.
Tree: `911c1caa70fb0b225c9f511a1e8a8a8a249903e0`.
Base: `fc364ed9706e3f8ceccba034bae375bbaded7e73`.

Verdict: PASS, with no open Critical, Important or Minor findings. This is
private stable-input Reader evidence. The candidate changes plans and tests;
its production source tree is byte-identical to the accepted base.

## Scope and result

The [facade inventory](plans/dtd-reader-quota-facade.md) maps all 40 applicable
settings: 32 `SessionLimits` fields, six parser `Limits` fields and two
`DecodeLimits` fields. The
[conformance matrix](plans/dtd-reader-quota-conformance.md) exercises them
through the private Reader constructor and `read` boundary. It covers exact
below/at/above ceilings, typed locations, accepted high-water positions, last
published origins, sticky failure, counter continuity and complete controls.

The final matrix has 94 focused groups. It closes constructor and BOM
precedence, physical versus decoded accounting, scanner and generated-unit
limits, catalog contexts, entity-frame continuity, reference/local source
switches, checked scratch arithmetic, aggregate/native work attribution, and
ownership/promotion error order. It does not claim exhaustive combinations,
allocator-exhaustion coverage, cross-frontend parity or complete fuzz coverage.

## Independent fault checks

Fifteen production faults were tested in a detached review checkout. The suite
caught native/global attribution changes, inclusive-ceiling errors, work and
frame resets at handoff, replacement-location corruption, renewed scanner
allowance, wrong scratch min/overflow arithmetic, fresh parser construction,
promotion-history resets, generated model-depth bypass and collapsed indirect
catalog identity. One initially surviving positive declaration-ceiling fault
led to a frozen cumulative case; its rerun then failed as required.

The plan also records an 83-group success-ceiling substitution: all paired
negative controls failed while 11 independent controls stayed green. That
substitution checks test discrimination and is separate from the production
fault campaign.

## Verification and limits

The candidate passes 94 focused groups, 764 library tests in debug and release,
five documentation tests, strict all-target Clippy, formatting and diff checks.
The final checkout was clean and its tracked files matched their Git blobs.

No production behavior, layout or allocation path changed. Public DTD
activation, UTF-16 ownership, DOM/Writer/FFI integration, allocator-failure
testing, coverage-guided fuzz closure and performance measurement remain open.
