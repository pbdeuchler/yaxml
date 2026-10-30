# yaxml

[![crates.io](https://img.shields.io/crates/v/yaxml.svg)](https://crates.io/crates/yaxml)

#### YetAnotherXML Crate

A safe, fast XML toolkit for Rust that behaves like libxml2.

- **Safe**: `#![forbid(unsafe_code)]`, one dependency (`memchr`), every
  resource bounded by default, and nothing loaded from disk or the network
  unless you pass a resolver.
- **Compatible**: libxml2 2.15.4 is the reference. Parse results, error codes
  and messages, serialized bytes and validation verdicts are tested against
  it, and option flags keep libxml2's names and numbers.
- **Fast**: faster than libxml2 on all 65 DTD-free DOM and streaming benchmark
  workloads ([benchmarks](docs/benchmarks.md)).
- **Broad**: borrowed and editable DOMs; pull, push and `xmlTextReader`
  streaming; XPath 1.0; DTD, XML Schema, RELAX NG and Schematron validation;
  C14N, XInclude, catalogs, HTML and writers.

```toml
[dependencies]
yaxml = "0.1"
```

Requires Rust 1.97 or later.

## Parse and query

```rust
use yaxml::{Document, xpath};

let document = Document::parse("<shop><item price='3'/><item price='4'/></shop>")?;
for item in document.root_element().children() {
    println!("{:?}", item.attribute("price"));
}

let total = xpath::XPath::compile("sum(//item/@price)")?;
let value = total.evaluate(&document, document.root().id(), &xpath::Context::new())?;
assert_eq!(value, xpath::Value::Number(7.0));
# Ok::<(), Box<dyn std::error::Error>>(())
```

`Document` borrows its input and never copies text that needs no decoding;
for UTF-16 or single-byte input, `Transcoded` decodes it once and
`Document::parse_transcoded` borrows that. `OwnedDocument` owns its strings,
decodes any input itself, and can be edited.

## Stream

Pull events from a slice, push byte chunks of any size, or use libxml2's
`xmlTextReader` cursor (`TextReader`) over any `BufRead`. Memory stays bounded
however large the document is.

```rust
use std::ops::ControlFlow;
use yaxml::{Event, PushParser, Reader};

let xml = b"<log><entry>a</entry><entry>b</entry></log>";

let mut reader = Reader::new(xml)?;
let mut entries = 0;
while let Some(event) = reader.read_event()? {
    if let Event::Start(element) = event {
        entries += usize::from(element.name.local() == "entry");
    }
}
assert_eq!(entries, 2);

let mut parser = PushParser::new();
for chunk in xml.chunks(5) {
    parser.feed(chunk, |_event, _origin| ControlFlow::Continue(()))?;
}
parser.finish(|_, _| ControlFlow::Continue(()))?;
# Ok::<(), yaxml::Error>(())
```

## Edit and save

```rust
use yaxml::{EditLimits, OwnedDocument, save};

let mut document = OwnedDocument::parse("<config><port>80</port></config>")?;
let root = document.root_element().id();
document.edit(EditLimits::default())?.append_comment(root, "edited")?;

let bytes = save::to_vec(&document, &Default::default())?;
assert_eq!(bytes, b"<?xml version=\"1.0\"?>\n<config><port>80</port><!--edited--></config>\n");
# Ok::<(), Box<dyn std::error::Error>>(())
```

## Validate

```rust
use yaxml::{Document, xsd};

let schema = xsd::Schema::parse(br#"<xs:schema xmlns:xs="http://www.w3.org/2001/XMLSchema">
  <xs:element name="n" type="xs:int"/></xs:schema>"#, "n.xsd")?;
assert!(xsd::is_valid(&schema.validate(&Document::parse("<n>42</n>")?)));
assert!(!xsd::is_valid(&schema.validate(&Document::parse("<n>x</n>")?)));
# Ok::<(), Box<dyn std::error::Error>>(())
```

DTD (`Document::validate`, `valid`), RELAX NG (`relaxng`) and Schematron
(`schematron`) work the same way. DTD, XML Schema and RELAX NG can also
validate a stream without building a tree.

## libxml2 options and errors

`ParseOptions` takes libxml2's `XML_PARSE_*` flags. A surface that does not
support a flag rejects it rather than ignoring it. An `Error` is small (a
`kind` and a byte `offset`), and `describe` expands it into libxml2's domain,
code, line, column and message. As in libxml2, a namespace error (an unbound
prefix, say) does not stop a parse: the name stays in no namespace and the
error is listed in `errors()`. `RECOVER` works on the DOMs and every stream,
which delivers the recovered tree's events and the same `errors()`.

```rust
use yaxml::{Document, LineIndex, ParseOptions, options};

let input = b"<r><a>text</b></r>";
let document = Document::parse_with(input, ParseOptions::new(options::RECOVER))?;
let messages: Vec<_> = LineIndex::new(input)
    .describe_all(document.errors())
    .into_iter()
    .map(|diagnostic| diagnostic.message)
    .collect();
assert_eq!(messages, ["Opening and ending tag mismatch: a line 1 and b"]);
# Ok::<(), yaxml::Error>(())
```

## Security defaults

| Threat                                       | Default                                                                                                |
| -------------------------------------------- | ------------------------------------------------------------------------------------------------------ |
| External entities (XXE), external DTDs       | Loaded only through a `Resolver` you supply, and only when the flags ask for them                      |
| Entity expansion ("billion laughs")          | Amplification and total-expansion limits, as in libxml2                                                |
| Deep nesting; huge names, attributes or text | Depth, size and count limits on every surface                                                          |
| Quadratic and backtracking inputs            | Linear parsing; content models, regular expressions and RELAX NG compile under work and memory budgets |
| Memory                                       | Every allocation that the input controls is charged to a budget                                        |

Each limit is a field of a `Limits` struct, and `options::HUGE` relaxes the
limits as libxml2's `XML_PARSE_HUGE` does. [docs/security.md](docs/security.md)
has the full table alongside libxml2's values. Every parser, validator and
writer has fuzz targets.

## Compatibility

yaxml reproduces libxml2's behavior, not its C API. For each feature,
[compatibility](docs/compatibility.md) records how it was checked against
libxml2 and every known difference. The [API mapping](docs/libxml2-api.tsv)
has one row per libxml2 function, and [conformance](docs/conformance.md)
covers the W3C test suite results.

Not yet supported: XML 1.1, and built-in decoding of the multi-byte encodings
that libxml2 reads only through iconv or ICU (Shift_JIS, EUC-JP, GBK and
others); a caller can supply them through `ParseOptions::converters`. The
[remaining work](docs/remaining-work.md) lists what is left.

## Cargo features

Everything is on by default (`full`). With `default-features = false` you get
the core: `Reader`, `PushParser`, `BufferedReader`, `Document`,
`OwnedDocument` and `Editor`, DTD processing and validation (`valid`),
diagnostics, `save`, `Writer`, `uri` and `chars` (about 41,300 of the
78,700 lines in `src/`). Then add what you need:

```toml
[dependencies]
yaxml = { version = "0.1", default-features = false, features = ["xpath"] }
```

| Feature | Enables | Implies | Lines |
| --- | --- | --- | --- |
| `xpath` | `xpath` compile and evaluation, `xpointer`, `pattern` | | 3,650 |
| `c14n` | `c14n` | `xpath` | 650 |
| `xinclude` | `xinclude`, the `XINCLUDE` parser flag | `xpath` | 1,350 |
| `catalog` | `catalog`, the `CATALOG_PI` parser flag | | 1,400 |
| `schemas` | `xsd`, `relaxng`, `schematron`, `schema_types`, `regexp` | `xpath` | 18,650 |
| `html` | `html`, `save::AS_HTML` | | 6,500 |
| `textreader` | `TextReader`, `Walker` (its RELAX NG and XSD hooks need `schemas`) | `schemas` | 2,350 |
| `textwriter` | `TextWriter` | | 1,200 |
| `debug` | `debug` (libxml2's debugXML dumps, `yaxmllint --debug`) | | 850 |
| `sax` | `sax` (SAX2-style callbacks, libxml2's `xmlSAXHandler`) | | 500 |
| `encodings` | UCS-4 and EBCDIC input and `save` output (the code pages of the macOS iconv) | | 400 |
| `full` | all of the above (default) | | |
| `allocator_api` | Rust 1.100+: `OwnedDocument` storage in any `Allocator` (`OwnedDocument::parse_in`); not in `full` | | |

Lines are the feature's own `src/` files, without what it implies. A flag
whose implementation is compiled out (`XINCLUDE`, `CATALOG_PI`,
`save::AS_HTML`) is `ErrorKind::UnsupportedOption`, and
`features::has` answers for the build. The `yaxmllint` binary needs `full`.

## Command line

`cargo install yaxml` installs `yaxmllint`, which accepts `xmllint`'s options
and matches its output ([yaxmllint](docs/yaxmllint.md)).

## Other languages

[`bindings/`](bindings) holds a C ABI (`yaxml-capi`), a Go package and an
`lxml.etree`-style Python package. They are developed alongside the crate but
not published yet.

## Benchmarks, differentials and fuzzing

Everything lives in this one repository. Plain `cargo test` needs only stable
Rust: no C compiler, libxml2, test suites or nightly. The rest is driven by
scripts at the root, which build two separate Cargo workspaces that `cargo
test` never touches: `harness/` (benchmarks, libxml2 differentials, the W3C
XML Test Suite run) and `fuzz/` (cargo-fuzz targets).

```sh
git clone https://github.com/pbdeuchler/yaxml && cd yaxml
cargo test                 # the library's own tests
./setup.sh                 # once: libxml2 2.15.4 oracle and the XML test suite
./oracle.sh                # every differential against libxml2, then XMLTS
./bench.sh tiny mixed      # yaxml vs libxml2 parse times
./fuzz.sh                  # list the fuzz targets
./fuzz.sh dom 600          # fuzz one for 10 minutes
./coverage.sh dom          # line/branch coverage of src/ from the corpus
```

Prerequisites: Rust via [rustup](https://rustup.rs) (rust-toolchain.toml pins
1.97.0). `setup.sh`, `oracle.sh` and `bench.sh` also need curl, cmake and a C
compiler. `fuzz.sh` and `coverage.sh` need `rustup toolchain install
nightly-2026-10-18 --component llvm-tools-preview` and `cargo install
cargo-fuzz --version 0.13.2 --locked`; `coverage.sh` also needs
[uv](https://docs.astral.sh/uv/). `pgo.sh` needs `llvm-profdata` (`rustup
component add llvm-tools`, or Xcode's). Every script takes `--help`.

`./setup.sh` downloads and checks (sha256) libxml2 2.15.4 and the W3C XML Test
Suite 20130923, builds the static libxml2 oracle in `../yaxml-oracle` (beside
the checkout; no iconv/ICU/zlib) and writes the suite catalog in
`../yaxml-suites`. Set `YAXML_ORACLE` or `YAXML_SUITES` to use other
locations; anything already present is reused.

| Command                        | What it does                                                                                                                                                                                                                | Results                                                                   |
| ------------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------- |
| `./oracle.sh [step...]`        | yaxml's ignored and env-gated differentials, the harness oracle tests, fuzz-seed replay, structural edits, and the conformance run, which fails on any regression of `harness/tests/fixtures/xmlconf-20130923-required.tsv` | `harness/results/oracle-<time>/`: one log per step, `xmlconf-results.tsv` |
| `./bench.sh [fixture...]`      | Matched parse benchmark, yaxml vs libxml2, in five modes; prints libxml2/yaxml time ratios (above 1 favors yaxml)                                                                                                           | table on stdout, raw samples in `harness/results/bench-<time>.txt`        |
| `./pgo.sh`                     | Profile-guided `yaxmllint`: instrumented build trained on the benchmark fixtures under 12 option sets, then rebuilt with the merged profile; prints instruction deltas against the plain build (macOS)                       | `harness/target/pgo/use/release/yaxmllint`                                |
| `./fuzz.sh [target [seconds]]` | Lists the fuzz targets, or fuzzes one (default 60 s)                                                                                                                                                                        | new inputs in the corpus, crashes in `fuzz/artifacts/<target>/`           |
| `./coverage.sh [target...]`    | Line/branch coverage of `src/` from the corpus                                                                                                                                                                              | `fuzz/coverage/report.txt`, `merged.lcov`                                 |

The repository carries a minimized corpus per target in `fuzz/corpus/`
(1.65 MB, 23,244 inputs). The full corpus is a separate repository: `./setup.sh corpus`
clones it into `fuzz/corpus-full/` (`YAXML_CORPUS_URL` overrides the
location), and `fuzz.sh` and `coverage.sh` use it, adding new inputs there,
whenever it is present. Without it, new inputs land in `fuzz/corpus/`; don't
commit those. Build output goes to `harness/target/` (`CARGO_TARGET_DIR`
overrides it) and `fuzz/target/` (`FUZZ_TARGET_DIR`).

Fuzz logic lives in the library's tests (`tests/support/fuzz/`, shared with
`tests/fuzz_smoke.rs`); [fuzz/README.md](fuzz/README.md) lists the targets and
campaign notes. To add a target:

1. Write the checks as a `pub fn exercise_<name>(data: &[u8])` in
   `tests/support/fuzz/` (a new module goes in `driver.rs` with a `#[path]`
   line), and call it from `tests/fuzz_smoke.rs` with a few inputs.
2. Add `fuzz/fuzz_targets/<name>.rs`, a copy of any other target calling your
   function, and a `[[bin]]` entry for it in `fuzz/Cargo.toml`.
3. Optionally add seeds to `fuzz/corpus/<name>/`, then `./fuzz.sh <name> 600`.
4. Fixed crashes: a minimized input goes to
   `tests/fixtures/fuzz-regressions/<name>/` (`fuzz/regressions` links there;
   map the directory to the function in `fuzz_regressions_replay` if it is not
   a plain XML target).

## License

MIT or Apache-2.0, at your option.
