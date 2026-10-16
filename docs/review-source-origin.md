# Holistic Review

**Plan:** [Source-origin boundary](plans/source-origin-boundary.md) at `5c01d9c`
**Base:** `4348e4992c479a967d1592300e156e91f94006c0`
**Files Changed:** 32 implementation/migration/documentation files at the author tip, plus the independently authored `tests/source_origin.rs` reviewed as the follow-up test commit
**Commits:** implementation `8a91809bc1a4882441df17e2cd90abe67374e3f2`; independent tests `49f9da88d25ac5c1355d0c710b08d29f88eb02c4`; private review tip `a75538af032b72f692333983341bc994d7ca128a`

## Summary

The scoped source-origin boundary is implemented as planned. `SourceOrigin` is public and opaque, all currently parsed origins expose exact physical input spans, and no entity/replacement representation is predeclared. Reader and push lifecycle state starts absent and retains the last delivered origin after EOF, parse failure, and callback cancellation. Push publishes the origin before invoking callbacks. Buffered delivery, borrowed DOM records, and owned parsed records use the same origin value; constructed owned records remain `None`. Element closure uses one fallible join, and missing owned provenance returns `InvalidState` without popping the parent or publishing a partial subtree.

All changed production, migration, fuzz, test, and documentation lines were reviewed against the plan. The API break is confined to the planned young metadata boundary. Parser grammar, decoding, physical progress, resource ceilings, semantic observers, stored payload lifetimes, Cargo dependencies, and unsafe-code policy are unchanged. The new public suite directly exercises all public origin getters; fuzz callback/range projections exercise `SourceOrigin::input_range`, and range convenience methods transitively exercise the corresponding origin getters. The existing fuzz inventory remains accurate for the exercised `event_range` paths, although naming the new origin aliases there would make it more exhaustive.

The author reports 514 passed and 9 ignored all-target tests, 13 doctests, strict Clippy, and formatting at the implementation tip. The final independent focused run passed 12 public tests plus both private join/provenance tests. An earlier draft's 64-test statement referred to the preceding construction-fuzz milestone and was not SourceOrigin coverage. Root's later fresh SourceOrigin run passed 64 tests against the changed production source and reported 6,936/7,406 regions, 406/459 functions, 4,797/4,970 lines, and 910/964 branches across all 17 production files, with no exclusions. This review makes no timing, XML conformance, DTD, replacement-provenance, ABI, or broad performance claim.

## Mutation Results

**Baseline:** GREEN. `cargo test --locked --test source_origin --test origin_lifecycle` passed 12/12; the two exact internal tests passed; focused strict Clippy and formatting passed. Final production tree `28ac931425855790f29b6ab25fa6683e5fcc3aa7` exactly matches the author tip after every mutation.

| Deliberate fault | Predicted detector | Result |
| --- | --- | --- |
| Publish push origin only after a callback returns successfully | Cancellation must retain the delivered Text origin, rather than the previous Start origin | Caught: expected `7..18`, observed retained `3..7` |
| Swap private diagnostic-start and cancellation-end endpoints | UTF-8 BOM Text cancellation must abort at physical end 18 | Caught: observed offset 7 instead of 18 |
| Clear Reader origin on successful EOF | Repeated EOF must retain the last End origin | Caught: observed `None` instead of `18..23` |
| Build streaming origins from decoded spans instead of physical source spans | UTF-16 every-split literal ranges | Caught: decoded `0..4`, `4..15`, `15..20` differed from physical `2..8`, `8..26`, `26..34` |
| Remove the forward-order check from element-origin joining | Private negative join cases | Caught: overlapping close origin was accepted |
| Replace the owned missing-provenance error with `expect` | Missing provenance must return typed `InvalidState` and preserve builder state | Caught: test observed the injected panic |

Each mutant was applied alone in the private review worktree, produced the expected failing assertion, and was restored before the next mutation. The final worktree is clean.

## Findings

### Critical

None.

### Important

None.

### Minor

None.

## Plan Adherence

| Requirement | Evidence | Status |
| --- | --- | --- |
| Opaque, copyable origin with optional exact input range | Private fields in `src/origin.rs`; literal public Copy/Send/Sync and retained-value test | Met |
| No speculative replacement/source-arena representation | Two-offset physical representation only; future entity contract stays documented | Met |
| Reader/Push optional latest origin and sticky lifecycle | Before-first-event, repeated EOF, parse-failure, cancellation, and terminal-call tests | Met |
| Push stores origin before callback | Code inspection and late-publication mutation failure | Met |
| Buffered callbacks and physical transport progress remain distinct | UTF-8/UTF-16 one-byte-buffer lookahead tests and unchanged progress accounting | Met |
| Borrowed DOM origins; owned parsed `Some`, constructed `None` | Literal nested DOM, source-drop, edit, removal, append, and fully constructed document tests | Met |
| Fallible, ordered element-origin join | Shared join implementation, negative overlap test, and missing-provenance atomicity test | Met |
| Preserve physical UTF-16 offsets | Three-encoding every-split tests and decoded-span mutant | Met |
| Migrate consumers without semantic-observer weakening | Full changed-file review, all-target author gate, direct fuzz projections, and unchanged observer/value checks | Met |
| Preserve resource/security boundaries | No new dependency or unsafe code; no new allocation or ambient I/O; private construction; typed failures | Met |
| Record actual layout rather than promise ABI | Author layout artifact records Reader/Push +8 bytes; node records, Event, BufferedReader, and DOM layouts unchanged | Met |
| Keep generated replacement provenance and DTD support out of scope | No DTD acceptance or generated-origin path added; limitation is explicit in plan and public docs | Met |

## Seven-Dimension Review

1. **Correctness:** literal spans, BOM handling, normalization independence, closure joins, cancellation offsets, and lifecycle retention agree across Reader, Push, Buffered, borrowed DOM, and owned DOM.
2. **Architecture:** the opaque boundary centralizes origin semantics without introducing unused variants or coupling transport progress to provenance.
3. **API and compatibility:** all intentional signature changes are migrated consistently; the public constructor remains private and constructed owned nodes are distinguishable through `Option`.
4. **Security and resources:** origins add no document-controlled allocation, recursion, I/O, or unchecked indexing. Invalid private builder states fail rather than panic in production.
5. **Testing:** independent literal goldens cover three encodings, every split, lookahead, failures, cancellation, edits, and construction; six meaningful faults were detected.
6. **Performance and layout:** the measured growth is limited to one word in Reader and PushParser. No benchmark or superiority claim is inferred from layout.
7. **Documentation and maintainability:** scope, current guarantees, future replacement constraints, physical offsets, and intentional API changes are documented. The implementation uses one small origin type and one join rule.

## Verdict

**PASS** — zero Critical, Important, or Minor findings in the reviewed source-origin boundary and independent public-test follow-up. Full generated replacement provenance remains a required later stage, exactly as the plan states.

## Evidence

- Author evidence index: `/private/tmp/yaxml-source-origin-checks.txt`
- Author layout comparison: `/private/tmp/yaxml-source-origin-layout-comparison.tsv`
- Independent public test source SHA-256: `09908ebe3ff866c2de29070c857d4e0396c4ed89aa875fe06ab579643c23aaae`
- Review baseline/final/mutant logs: `/private/tmp/yaxml-source-origin-review-*.log`
- Review worktree: `/private/tmp/yaxml-source-origin-review`

## Integration evidence review

Root reviewed the source, independent tests, migrated observers and complete
scoped report. The plan/implementation/public-tests series was cherry-picked
onto main `3726b3d` as `e108b49`, `b8429ab` and `7ac460b`. The inventory-only
correction `b2f2020` names the new origin methods; production remains the same
reviewed tree `28ac931425855790f29b6ab25fa6683e5fcc3aa7`.

The [final validation](validation-source-origin.md) records 535 passing test
executions, nine ignored tests, thirteen doctests, strict Clippy/formatting,
complete byte-identical XMLTS results and fresh source-complete fuzz profiles.
The new defensive failure paths remain in the fuzz denominator; ordinary
negative tests are not counted as fuzz coverage.

Measured node-record/Event sizes remain unchanged, with Reader/Push growing one
word. The independently audited 65-workload matrix retains exact observations
and identities: no median slowdown exceeds two percent, and text push/seven-byte
primary time falls about 6.5%. This is a local Rust A/B result, not proof of
libxml2 superiority or a generally faster algorithm.

**Integration PASS.** All boundary-stage gates are met. Replacement occurrence
identity/coordinates, entity-frame grammar boundaries, generated text provenance,
DTD activation and reference-preserving modes remain required work. The
[next representation outline](plans/dtd-representation.md) records logical
declaration events and compact units without implementing unused public variants.
Full libxml2 behavior, allocator support on the target compiler, language
adapters and 100% fuzz coverage remain open.
