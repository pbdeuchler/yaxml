# yaxml agent notes

Pure-Rust libxml2 2.15.4 rewrite. `#![forbid(unsafe_code)]`; memchr is the only library dep (fuzz-only deps live in `fuzz/`, its own workspace). FFI bindings go in separate crates (docs/bindings.md, owner sign-off needed).

## Layout
- One engine (src/parser.rs, src/tokenizer, src/stream) feeds Reader (slice), PushParser, BufferedReader, TextReader, Document (borrowed DOM), OwnedDocument (arena, `OwnedNodeId`). DTD-free input stays on the plain token loop; DTD stack = src/dtd*.
- `ParseOptions` (src/options.rs) is the single entry for flags (libxml2 `XML_PARSE_*` values), `Limits`, `DecodeLimits`, `base`, `Resolver`. Default = libxml2 default (entity-ref nodes kept, nothing loaded). Unimplemented flags -> `ErrorKind::UnsupportedOption`, never silently ignored.
- Crate-private `Navigate` (src/xpath/tree.rs) is shared by XPath, C14N, valid, relaxng, xsd, pattern, schematron.
- relaxng: one `Engine<I>` (src/relaxng.rs: start/flush/text/end/finish) driven by the DOM walk and by `relaxng::Stream` (events; used by `TextReader::relaxng_set_schema`). `Deriver` holds `Arc<Arena>`/`Arc<Tables>` so `Stream` has no lifetime. No subtree buffering (libxml2 FullElement unneeded).
- XSD instance validation is one event engine (src/xsd/validate.rs `Validator`: start/text/end/other); tree `run` and public `xsd::StreamValidator` both feed it. Stream == DOM diagnostics is tested (tests/xsd_stream.rs, suite, fuzz/xsd.rs). `Schema` = `Arc<Compiled>` (Deref), so `StreamValidator`, `Validator`, `Engine` and `pattern::StreamMatcher` (`Arc<Stream>`) have no lifetime. `TextReader.validators` (relaxng + xsd) is the one feeding point (`fetch`, `skip_entity`, Doctype -> `set_dtd`); `xsd_set_schema`/`xsd_diagnostics`; `is_valid` = any set and none failing.
- No I/O without a `Resolver` (src/resolve.rs); `FileResolver` is root-confined. Entity amplification follows libxml2 (`max_entity_amplification`, default 5).

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
Oracle differentials (`*_oracle.rs`, `--ignored`) need env vars: `YAXML_XMLLINT`, `YAXML_URI_HELPER`, `YAXML_LIBXML2_SOURCE`.

## Fuzz
- Targets `fuzz/fuzz_targets/*.rs` are thin wrappers over `fuzz/driver.rs` `exercise_*`. Corpus `fuzz/corpus/<t>` (files <= 4 KiB, edge-only cmin), `fuzz/regressions/<t>` replayed by tests/fuzz_smoke.rs, unfixed findings in `fuzz/open/`.
- Coverage: `fuzz/coverage.sh` -> per-target lcov unioned by `coverage_merge.py` (multi-object llvm-cov undercounts). Campaign flags: `-fork=1 -timeout=2`.

## Conventions
- Deliberate libxml2 divergences are listed in the module doc (e.g. docs/xsd.md "Differences"). libxml2 quirks are matched unless they are bugs that break spec validity or security.
- Table-driven tests, happy + sad paths. Update docs/compatibility.md rows with each feature.
