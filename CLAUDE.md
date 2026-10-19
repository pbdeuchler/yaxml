# yaxml agent notes

Pure-Rust libxml2 2.15.4 rewrite. `#![forbid(unsafe_code)]`; memchr is the only library dep (fuzz-only deps live in `fuzz/`, its own workspace). FFI bindings go in separate crates (docs/bindings.md, owner sign-off needed).

## Layout
- One engine (src/parser.rs, src/tokenizer, src/stream) feeds Reader (slice), PushParser, BufferedReader, TextReader, Document (borrowed DOM), OwnedDocument (arena, `OwnedNodeId`). DTD-free input stays on the plain token loop; DTD stack = src/dtd*.
- `ParseOptions` (src/options.rs) is the single entry for flags (libxml2 `XML_PARSE_*` values), `Limits`, `DecodeLimits`, `base`, `Resolver`. Default = libxml2 default (entity-ref nodes kept, nothing loaded). Unimplemented flags -> `ErrorKind::UnsupportedOption`, never silently ignored.
- Crate-private `Navigate` (src/xpath/tree.rs) is shared by XPath, C14N, valid, relaxng, xsd, pattern, schematron.
- Feature `allocator_api` (Rust 1.100): `OwnedDocument<A = Global>`/`OwnedNode<'d, A>`/`Editor<'d, A>`; nodes+attributes in `allocator::Vec<_, A>` (one clone of `A` each), `parse_in`/`create_in`/`allocator()`. src/allocator.rs is the only cfg switch (off = sealed stand-in, MSRV 1.95, API unchanged). Pool/ids/dtd/errors/scratch stay global. Generic consumers: `Navigate` users + Writer (`save::Walk`); `save::Tree`/`to_vec`/html/xinclude Global-only (generic public `Tree` breaks `Tree::Borrowed(..)` inference). `parse_in` rejects XINCLUDE. Keep new constructors in `impl OwnedDocument` (Global) so `OwnedDocument::parse(..)` infers. Derive Copy/Clone on A-generic views manually (derive adds `A: Copy`).
- relaxng: one `Engine<I>` (src/relaxng.rs: start/flush/text/end/finish) driven by the DOM walk and by `relaxng::Stream` (events; used by `TextReader::relaxng_set_schema`). `Deriver` holds `Arc<Arena>`/`Arc<Tables>` so `Stream` has no lifetime. No subtree buffering (libxml2 FullElement unneeded).
- XSD instance validation is one event engine (src/xsd/validate.rs `Validator`: start/text/end/other); tree `run` and public `xsd::StreamValidator` both feed it. Stream == DOM diagnostics is tested (tests/xsd_stream.rs, suite, fuzz/xsd.rs). `Schema` = `Arc<Compiled>` (crate-private, fields via `schema.data`), so `StreamValidator`, `Validator`, `Engine` and `pattern::StreamMatcher` (`Arc<Stream>`, state split in `Cursor` so no per-event clone) have no lifetime. `TextReader.validators` (relaxng + xsd) is the one feeding point (`fetch`, `skip_entity`, Doctype -> `set_dtd`); `xsd_set_schema`/`xsd_diagnostics`; `is_valid` = any set and none failing.
- Streaming hot path: tokenizer `step` (src/tokenizer.rs) is one memchr step per lexical state (no per-byte calls; a markup prefix classifies all its available bytes in one step); `Stream::advance`/`Tokenizer::advance` are `inline(always)`, token runs append within spare capacity without the checked growth path; plain UTF-8 bypasses the decoder (`Stream::advance`). Stream `ParserState` is `lending_namespaces`: names get placeholder `LENT`, `ParserState::lend` fills them from bindings at delivery, closed scopes unwind at the next `process_token` (`settle`); DTD handover calls `stop_lending`. Perf method/table: docs/benchmarks.md (matched driver + `/usr/bin/time -l` instruction counts under host load). PushParser holds only the session (failure/finish/`max_input_bytes` live in `StreamingSession`). Measured dead ends (don't retry): in-place `drive_chunk` below 32 bytes (+6% instr, tails rarely hold a token), scalar loops instead of memchr on short haystacks (+2%), `inline(always)` on `process_token` (-2% instr but +6-11% cycles). Exact per-feed instruction traces: lldb single-step script in docs/benchmarks.md method.
- Attribute entity refs without NOENT (libxml2 attr children): `dtd_tag::kept_form` -> `PreparedAttribute.kept` -> `Budget::kept()` (last start tag, by final attribute position; DOM builders drain it) -> sorted side tables `Document.kept` / `OwnedDocument.kept` (allocator Vec, pool ranges); empty = zero cost. Public `unexpanded_attribute(pos)`; `save` (`Walk::kept_form`) and yaxmllint `--xpath` write it; `attributes()` values stay substituted (XPath/valid/C14N). ATTLIST defaults carry `StoredDefault::Value.kept` (DTDATTR). Writer writes substituted. Owned `set_attribute_value` drops the form.
- No I/O without a `Resolver` (src/resolve.rs); `FileResolver` is root-confined. Entity amplification follows libxml2 (`max_entity_amplification`, default 5).
- `RECOVER` (DOMs only): error sites call `Budget::recover` (src/parser.rs; strict -> Err, recover -> record + libxml2 repair at that site). Unrepaired error ends the parse: DOM builders call `Budget::conclude` and close open elements. Limits/loops/alloc stay fatal; no element -> Err(first error). Divergences: docs/compatibility.md#recovery-recover. Oracle: `recover_against_xmllint` (YAXML_XMLLINT, YAXML_SUITES).

## Repo/process
- Canonical repo: bare `../yaxml.git`; worktrees `../yaxml-wt/<name>`, main in `../yaxml-wt/main`. Visible workspace dir has no `.git`.
- Process: docs/process.md (one slice, one review; measured claims). Legacy evidence docs: docs/archive (not maintained).
- Oracle: `../yaxml-oracle/build/xmllint`, `libxml2.a`, source `../yaxml-oracle/libxml2-2.15.4`. URI helper build: docs/uri.md. XMLTS: `../yaxml-suites/xmlconf` + `xmlconf-catalog.tsv`.
- Python: run with `uv run --no-project` (hook rejects python3).

## Gate (every merge)
```
cargo fmt --check && CARGO_TARGET_DIR=/private/tmp/<t> cargo clippy --all-targets -q -- -D warnings && cargo test --all-targets -q && (cd fuzz && cargo +nightly-2026-06-06 fuzz build -O)
/private/tmp/<t>/release/examples/xml_conformance --manifest ../../yaxml-suites/xmlconf-catalog.tsv --suite-root ../../yaxml-suites/xmlconf --expected-cases 2585 --required-pass tests/fixtures/xmlconf-20130923-required.tsv   # required_pass_regressions=0
```
Feature gate (beta until 1.100 ships): `cargo +beta clippy --all-targets --features allocator_api -- -D warnings && cargo +beta test --all-targets --features allocator_api`.
Oracle differentials (`*_oracle.rs`, `--ignored`) need env vars: `YAXML_XMLLINT`, `YAXML_URI_HELPER`, `YAXML_LIBXML2_SOURCE`.

## Fuzz
- Targets `fuzz/fuzz_targets/*.rs` are thin wrappers over `fuzz/driver.rs` `exercise_*`. Corpus `fuzz/corpus/<t>` (files <= 4 KiB, edge-only cmin), `fuzz/regressions/<t>` replayed by tests/fuzz_smoke.rs, unfixed findings in `fuzz/open/`.
- Coverage: `fuzz/coverage.sh` -> per-target lcov unioned by `coverage_merge.py` (multi-object llvm-cov undercounts). Campaign flags: `-fork=1 -timeout=2`.

## Conventions
- Deliberate libxml2 divergences are listed in the module doc (e.g. docs/xsd.md "Differences"). libxml2 quirks are matched unless they are bugs that break spec validity or security.
- Table-driven tests, happy + sad paths. Update docs/compatibility.md rows with each feature.
