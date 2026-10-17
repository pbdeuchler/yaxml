# Private DTD semantics and source-frame review

Reviewed private freeze: `dbb5a3879a857a47ae844fe65dd5e669bfa514a1`.
Source tree: `1d2d1a1dad4453ab69ae71251bad7add16745beb`.
Worktree: `/private/tmp/yaxml-dtd-integration`, branch
`integrate/dtd-prototype`. Main still has the production source of `f2bd043`;
these modules remain private and test-only. This review extends the
[grammar/scanner review](review-dtd-prototypes.md), without enabling DTD parsing.

## Implemented boundaries

The [catalog](plans/dtd-catalog.md) holds separate first-binding GE/PE indexes,
stable entity IDs and ordered first-binding attribute declarations. It retains
processed replacement text and normalized defaults, rather than another raw DTD
tree. Defaults use declarations already read and never rebind retroactively.
Unknown or unread dependencies have explicit outcomes; they cannot authorize a
future strict controller to publish an empty default or report parse success.

EntityValue construction retains all general-reference spellings and decodes
numeric references. Attribute use expands general references with iterative
frames. Output is not rescanned within either phase. Physical CRLF/CR processing
occurs once; replacement-source numeric CR remains CR. Declared non-CDATA types
collapse U+0020 after reference processing, preserving direct numeric TAB/LF/CR.
Unused replacement text is not eagerly checked for content well-formedness,
unresolved references or cycles: those checks apply when included.

The [source-frame core](plans/dtd-entity-frames.md) stores IDs, cursors, lengths
and boundaries, borrowing replacement text only during an operation. Each
successful reference gets a fresh invocation ID. Exact replacement provenance
includes that ID, local bytes and the outer physical reference span. Joins across
sources or invocations become explicitly Mixed, without a history arena.

Depth equality at frame exit cannot detect `</outer><b>` closing a caller's
element and restoring the depth. A pre-end-tag guard therefore rejects closure
at or below the active frame's entry depth. The future controller must call it
before accepting or publishing an end tag, and must separately validate complete
tokens within each source. The frame core alone does not validate XML markup.

Borrowed declaration units can be promoted to owned units using the parser's
shared owned-byte budget. Already-owned raw strings and validated token vectors
move without copies. Two composition tests schedule a PE-generated declaration,
preserve its numeric CR through catalog application, and distinguish same-name
GE/PE declarations and repeated invocation provenance.

All growth is fallibly reserved and checked against logical ceilings. Catalog
limits separately bound declarations, bindings, retained strings, one output
value, cumulative emitted bytes, work, references and depth. Frame limits also
charge empty and failed references. These bounds do not claim exact RSS or
allocator-exhaustion coverage. Fatal catalog mutation is sticky; read-only
prefix bindings after failure are not a successful document.

## Findings and independent review

Two catalog findings were fixed before this freeze: semantic external IDs had
retained quotes, and unavailable PE construction had not set the later-declaration
barrier. Regressions cover both. Unquoted IDs undergo physical EOL handling once;
public-ID matching normalization and resource resolution remain unimplemented.

Independent source review found no additional blocking defect. The test catalog
was derived from XML before adapting it to the author's API. There are 19
independent catalog groups and 26 author groups, plus 12 independent and 17
author frame groups. The complete private foundation has 129 focused groups,
including grammar, scanner, ownership and composition cases.

Root deliberate faults caught overly broad whitespace collapse, acceptance of
literal less-than from an attribute replacement, and omitted pre-collapse byte
charges. Independent catalog review caught first-binding replacement, conflated
normalization phases, both depth-boundary directions, broken unread barriers
and repeated physical normalization. Frame faults covered the pre-end guard,
identity reuse, UTF-8 boundaries, cycles, empty-reference accounting, parent
cursor publication and joins across distinct invocations. Ownership faults
caught omitted budget charges and unwanted raw/token copies. Every counted
fault failed a runtime assertion after a passing clean baseline; source was
restored. Compilation failures were not counted as detections.

Fresh root checks on the exact clean freeze passed 664 tests with nine ignored
across 44 ordinary result groups, all 13 doctests, strict all-target Clippy,
formatting and diff checks. The existing 64 fuzz-smoke tests are included but
do not fuzz the private DTD modules. Benchmark smoke binaries ran without a
comparative timing claim. All 296 tracked files matched their Git blobs.
Runner `/private/tmp/yaxml-dtd-root-semantic-final-checks.sh` checked the head
and clean status before and after; handle `32767` is terminal with exit zero.
Logs use `/private/tmp/yaxml-dtd-root-semantic-final-*.log`. Compiler: Rust/Cargo
1.95.0, LLVM 22.1.2, aarch64-apple-darwin.

Evidence includes `/private/tmp/yaxml-dtd-root-{frame,ownership,catalog}-faults.tsv`,
the separate independent test commits `fdb14c6` and `64a6464`, and the frozen
semantic catalog `/private/tmp/yaxml-dtd-semantic-spec-catalog.md`, SHA-256
`e2e55e98a81dc6b850cfdd9435ab59a6925d18535bc23f7d8839d7918ad12fca`.

## Reference checks

Primary rules are [attribute normalization](https://www.w3.org/TR/2008/REC-xml-20081126/#AVNormalize),
[references](https://www.w3.org/TR/2008/REC-xml-20081126/#sec-references),
[entity processing](https://www.w3.org/TR/2008/REC-xml-20081126/#entproc) and
[predefined declarations](https://www.w3.org/TR/2008/REC-xml-20081126/#sec-predefined-ent).
The private strict predefined-declaration diagnostic is a compatibility
difference, not a newly invented well-formedness or validity constraint.

The root adapted the previously reviewed SAX2 helper for default attributes,
keeping context user data, default entity callbacks, NOENT, NONET, NO_XXE and a
per-context deny loader. Nineteen literal fixtures ran through DOM, Reader and
SAX (seven-byte SAX feeds): 57 commands and 57 exact repeats. All resource-loader
counts were zero; compile/runtime libxml2 versions were both 21504.

| Case | Observed pinned libxml2 behavior |
| --- | --- |
| Direct or indirect default reaches a future/missing GE | Strict failure; later declarations do not repair it |
| Both referenced declarations precede the default | Normalized value `v` |
| Ignored duplicate default references a missing GE | Still fails |
| External/unread dependency | Error diagnostic; some calls return a partial DOM with empty value, which the strict helper rejects |
| Entity replacement retains `AT&amp;T;` | Attribute value `AT&T;`, without rescanning `T;` |
| Numeric less-than constructed inside a GE | Fails when used in an attribute |
| Retained `&lt;` inside a GE | Produces allowed attribute data `<` |
| Unused unresolved, cyclic or malformed replacement | Accepted without diagnostics |
| Malformed amp/lt declaration | Warning and fixed built-in value; private catalog instead rejects |

The raw amp/lt declarations require double escaping because section 4.6
constrains the processed replacement text to be a character reference. For
example, raw `&#38;#60;` constructs replacement `&#60;`; raw `&#60;` constructs
literal `<` and does not satisfy that form.

Probe directory: `/private/tmp/yaxml-dtd-default-order-probes`. Exact command
records, stdout/stderr, repeats, fixture generator and before/after input pins
are retained there. SHA-256 identities:

| Artifact | SHA-256 |
| --- | --- |
| `trace.c` | `82ccb06a2a56df3d60e1fe2bd812a795aac7deb40705c58438a0f4047a6bcfad` |
| Strict helper | `c98031ba888a174427a829b2ded76d214e8d997aff21153b9029b942c77d69fc` |
| `summary.tsv` | `264f3c0cd70be635736bd3e75d32e9b75aca5411224e1b348d67bb135a1c30e1` |
| `commands.json` | `951813a70a45fb0d86fecac7bcd4779aafd105f367635e9b38581b7801bc8ff1` |
| Input pins | `a6da39ea433b554214dfa7dcc2f8654de6f2a1ab73f1bf14ab7462bad761ceea` |
| Existing linked `libxml2.a` | `b48612c66e7d4d7fae1ceb7db9a0d3e288085bc91b560f037862607eccac1ab0` |

## Remaining activation work

The incremental controller is next: enforce family/context gates, map UTF-16
physical spans, expand complete internal PE fragments, retain ordered logical
DTD output, and leave the document-body suffix untouched. It must integrate GE
content frames, default attributes before namespaces, standalone provenance,
replacement/Mixed origins, both DOM text joins, public events and serialization
across all five frontends. No empty-only public DTD acceptance is planned.

Public activation still requires source-complete fuzz targets and coverage,
XMLTS deltas, layouts, serialization and paired performance checks. Existing
main coverage and conformance measurements exclude these private modules.
Full libxml2 compatibility, allocator integration, 100% fuzz coverage and
performance superiority remain open goals.
