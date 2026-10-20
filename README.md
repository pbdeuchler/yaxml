# yaxml

A safe, fast XML toolkit for Rust that behaves like libxml2.

- **Safe**: `#![forbid(unsafe_code)]`, one dependency (`memchr`), every
  resource bounded by default, and nothing loaded from disk or the network
  unless you pass a resolver.
- **Compatible**: libxml2 2.15.4 is the reference. Parse results, error codes
  and messages, serialized bytes and validation verdicts are tested against
  it, and option flags keep libxml2's names and numbers.
- **Fast**: faster than libxml2 on 63 of 65 DOM and streaming benchmark
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

`Document` borrows its input and never copies text that needs no decoding.
`OwnedDocument` owns its strings and can be edited.

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

| Threat | Default |
| --- | --- |
| External entities (XXE), external DTDs | Loaded only through a `Resolver` you supply, and only when the flags ask for them |
| Entity expansion ("billion laughs") | Amplification and total-expansion limits, as in libxml2 |
| Deep nesting; huge names, attributes or text | Depth, size and count limits on every surface |
| Quadratic and backtracking inputs | Linear algorithms; regular expressions and RELAX NG run on bounded automata |
| Memory | Every allocation that the input controls is charged to a budget |

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

Not yet supported: XML 1.1, encodings that libxml2 reads only through iconv or
ICU (Shift_JIS, EUC-JP, GBK and others), and the SAX2 handler table. The
[remaining work](docs/remaining-work.md) lists what is left.

## Cargo features

- `allocator_api` (Rust 1.100+): place `OwnedDocument` storage in any
  `Allocator` (`OwnedDocument::parse_in`).

## Command line

`cargo install yaxml` installs `yaxmllint`, which accepts `xmllint`'s options
and matches its output ([yaxmllint](docs/yaxmllint.md)).

## Other languages

[`bindings/`](bindings) holds a C ABI (`yaxml-capi`), a Go package and an
`lxml.etree`-style Python package. They are developed alongside the crate but
not published yet.

## Benchmarks and fuzzing

The libxml2 comparison benchmarks, fuzz targets, corpora and coverage tooling
live in [yaxml-harness](https://github.com/pbdeuchler/yaxml-harness). Each is
one command.

## License

MIT or Apache-2.0, at your option.
