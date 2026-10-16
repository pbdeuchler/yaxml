# Holistic Review

## Plan / Base / Files Changed / Commits

- Plan: `docs/plans/dtd-foundations.md`, limited to the ownership-foundation slice. DTD parsing and entity behavior are outside this review.
- Base: `2609aeff4c5287b5fbbdadd15eb55b1cbd296aa0`.
- Reviewed tip: `e3dafa83ca2f7b2430a0613824010c337b8386ec`.
- Review scope: every changed non-production file, read in full, plus the complete non-production diff. Production changes were assigned to the independent source reviewer.
- Commits in the reviewed chain:
  - `45c78db` — DTD ownership and semantic-foundation plan.
  - `1c03b46` — production ownership implementation; excluded from this scoped file review.
  - `570119d` — public ownership contract tests.
  - `e3dafa8` — name and owned-PI fuzz coverage.

The non-production diff contains 42 files and 1,062 insertions / 449 deletions: 3 documentation files, 3 benches, 3 example/support files, 9 fuzz files, and 24 test files.

Reviewed documentation:

- `README.md`
- `docs/owned-dom.md`
- `docs/plans/dtd-foundations.md`

Reviewed benches and example support:

- `benches/mutation.rs`
- `benches/pull.rs`
- `benches/structural_mutation.rs`
- `examples/support/infoset.rs`
- `examples/support/parse_observer.rs`
- `examples/support/structure.rs`

Reviewed fuzz files:

- `fuzz/README.md`
- `fuzz/construction.rs`
- `fuzz/construction_validation.rs`
- `fuzz/corpus/name-owned-views.txt`
- `fuzz/driver.rs`
- `fuzz/mutation.rs`
- `fuzz/names.rs`
- `fuzz/owned.rs`
- `fuzz/writer.rs`

Reviewed tests:

- `tests/buffered.rs`
- `tests/conformance.rs`
- `tests/dom.rs`
- `tests/fuzz_smoke.rs`
- `tests/internal/editor.rs`
- `tests/internal/owned.rs`
- `tests/internal/owned_structure.rs`
- `tests/internal/parser.rs`
- `tests/internal/serialize.rs`
- `tests/internal/writer.rs`
- `tests/io.rs`
- `tests/mutation.rs`
- `tests/name_ownership.rs`
- `tests/owned.rs`
- `tests/owned_construction.rs`
- `tests/owned_links.rs`
- `tests/owned_structure.rs`
- `tests/reader.rs`
- `tests/serialize.rs`
- `tests/serialize_io.rs`
- `tests/support/capture.rs`
- `tests/utf16.rs`
- `tests/writer.rs`
- `tests/writer_io.rs`

## Summary

**Scoped PASS.** The non-production migration follows the ownership-foundation plan and preserves the strength of the existing observers and tests.

The mechanical field-to-getter migrations retain all four name observations: qualified spelling, prefix, local part, and expanded namespace. Parsing, owned-tree, streaming, Writer, UTF-16, serialization, construction, mutation, and external-oracle projections continue to compare the same semantic data. The parse observer's literal transcript still fixes the same six counters and hash input, the structural oracle still records names, namespace URIs, attribute order and values, and the pull/structural benchmarks retain their previous black-box observations and checksum inputs.

The formerly forgeable invalid `Name` metadata tests were not silently discarded. Lexically invalid spellings now fail through `Name::new` with exact offsets, while Writer tests still reject valid lexical names carrying an empty, absent, undeclared, or mismatched namespace. There is no public or test-only unchecked constructor in the reviewed files. Dynamic fuzz strings use fallible construction; `unwrap()` is confined to fixed literals, previously parsed names, or independently constructed valid name parts.

The infoset helper replaces its invalid empty-name sentinel with `Option<Name>`. Elements and attributes still project qualified/local/namespace fields; comments and text project empty name fields; PI targets are validated and projected as names. Its existing independent query/framing tests all pass, so the migration does not drop an oracle field.

The new name fuzzer has 28 literal QName cases: 10 valid and 18 invalid, with exact UTF-8 byte offsets. It exercises borrowed and owned QName/namespace inputs, pointer preservation, arbitrary caller strings derived from the first 96 raw bytes (lossy UTF-8 output can reach 288 bytes), getter recomposition, and an owned PI Writer/Reader round trip. The literal matrix executes all four namespace forms for every case. The five-byte seed is exactly `cf 80 3a ce b4` (`π:δ`) with no trailing newline. The driver calls the name target before grammar/encoding dispatch; its instrumentation test includes empty, malformed, UTF-16, Unicode, and ordinary XML inputs.

The Writer fuzz migration intentionally stops manufacturing impossible private-field mismatches. It preserves reachable checks for undeclared prefixes, namespace metadata mismatches, invalid caller QNames, forbidden delimiters, atomic output position, and terminal failure. Constructor rejection skips the Writer operation and leaves the writer usable, which matches the new API boundary.

### Seven dimensions

1. **Correctness:** Existing semantic comparisons retain their name, namespace, PI, attribute, topology, span, and byte-output observations. Cow conversions use `as_ref`, `into_owned`, or getter-returned `&str` without changing values.
2. **Security:** Caller-created names are validated before they can reach Writer state. Exact invalid-name offsets, namespace-context rejection, no-output-on-error, and fuzz bounds remain asserted. No unchecked construction seam appears in non-production code.
3. **Performance:** Bench observers perform the same logical work after replacing fields with getters. This review ran no benchmark and makes no speed or regression claim. Layout and timing evidence require their separate gates.
4. **Maintainability:** The migration is mostly direct getter use. `Option<Name>` in the infoset projection is clearer than a forged invalid sentinel and makes unnamed node kinds explicit.
5. **API/design:** Documentation accurately describes validated construction, borrowed versus moved input, getter lifetimes, preservation of `Some("")`, Writer scope validation, and Cow PI targets. Declaration borrowing remains unchanged.
6. **Tests/fuzzing:** Public tests cover seven ownership cases and five parsing/streaming surfaces. Existing Writer and oracle tests remain meaningful. The fuzz target has finite per-input work and a deterministic complete literal catalog.
7. **Operational/documentation:** The fuzz inventory identifies the new target without claiming complete coverage. No DTD support, compatibility completion, or performance superiority is claimed by these changes.

## Mutation Results

Production mutation testing was deliberately not duplicated here because `/root/mutation_holistic` owns the source holistic review and fault gates. This review remained read-only as requested. The focused baseline checks were:

| Command | Result |
| --- | --- |
| `cargo test --test name_ownership` | 7 passed |
| `cargo test --test fuzz_smoke name` | 15 passed, 46 filtered |
| `cargo test --test writer name` | 14 passed, 26 filtered |
| `cargo test --example parse_bench canonical_transcript_has_independent_counts_and_hash` | 1 passed, 7 filtered |
| `cargo test --test oracle_infoset` | 11 passed |
| `cargo test --example structure_oracle` | 7 passed |
| `git diff --check 2609aef..HEAD` | passed |

These are test executions per command; several fuzz-driver unit tests are compiled into more than one integration target, so their totals must not be summed as a unique-test count.

## Findings

### Critical

None.

### Important

None.

### Minor

None.

## Plan Adherence

| Requirement | Evidence | Result |
| --- | --- | --- |
| Replace public `Name` fields with validated construction and getters | README, owned-DOM docs, all migrated tests/benches/examples | Pass |
| Preserve borrowed input and move owned buffers | Public pointer tests and fuzz pointer checks | Pass |
| Preserve namespace metadata, including `Some("")` | Public literal cases and Writer context tests | Pass |
| Keep namespace-context validation in Writer | Empty/missing/wrong binding cases still require `Namespace` and zero output | Pass |
| Make PI targets ownership-capable without changing declarations | Owned PI literal round trip across Reader, DOM, owned DOM, Push, Buffered; declaration borrow assertion | Pass |
| Remove forged invalid metadata without losing reachable negative coverage | Constructor offset tests plus Writer namespace mismatch tests | Pass |
| Preserve oracle observations | Parse transcript, infoset framing, structural snapshot, and getter-equivalent benchmark observations | Pass |
| Add bounded deterministic fuzzing before encoding dispatch | 28-case catalog, 96-raw-byte arbitrary window with a 288-byte lossy UTF-8 maximum, dispatch instrumentation, five-byte corpus seed | Pass |
| Do not add DTD acceptance in the ownership foundation | No reviewed tool/test/document claims or exercises DTD acceptance | Pass |

## Verdict

**PASS** for the changed non-production migration, tooling, tests, and fuzz files at `e3dafa83ca2f7b2430a0613824010c337b8386ec`. No blocker or follow-up defect was found in this scope. This verdict does not include production-source review and does not make a performance claim.
