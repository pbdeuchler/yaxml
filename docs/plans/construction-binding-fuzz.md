# Construction binding and limit precedence fuzzing

Base: `4348e4992c479a967d1592300e156e91f94006c0`.
Production source remains unchanged. Extend the independent construction model
to exercise matching-prefix compatibility and its resource/error precedence.

Each input selects one of four surfaces (root creation or append to a UTF-8,
UTF-16LE or UTF-16BE parsed parent) and one of three fixed variants. The spec
declares default namespace `v`, prefix `r=u`, an automatic element `{u}e`, and
an exact `r:a` attribute. The explicit different default forces prefix lookup.

For an attribute with no namespace, a two-byte name ceiling rejects generated
`ns1` at element-prefix byte two before the later namespace failure. A three-byte
ceiling permits generation and reaches `Namespace` at attribute item two. A
compatible attribute in namespace `u` must reuse `r` under the two-byte ceiling.
Predict each error before attempting the operation; retain exact locations.

Check literal successful names, declaration order, attributes, topology and
serialization. Failed appends must preserve complete snapshots/output and allow
a plain-child retry through the same editor. Failed creation has no editor, so
pair it with an independent valid constructor under the same ceiling.

Prove all twelve selections through actual top-level fuzz dispatch, with
thread-local test-only observations. Pin literal encoding prefixes and source
ranges so collapsing the input surfaces fails a test. Retain one explicit seed
and include it in replay/truncation schedules. Each selected case has three
items, one attempted operation and at most one recovery; arbitrary input must
not increase that work.

Review independently, deliberately break compatibility, dispatch, encoding and
exact diagnostics, restore every fault, then run fresh fuzz-only source coverage
with the complete production denominator. Report remaining gaps without adding
production hooks or exclusions. This is a finite regression model, not complete
namespace/limit state-space exploration or a sustained campaign.
